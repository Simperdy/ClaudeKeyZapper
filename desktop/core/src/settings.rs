//! Writes a profile into `<root>/.claude/settings.local.json` (the highest-precedence non-managed layer)
//! and reverts exactly the values it wrote. Verified behaviour: see docs/feasibility.md.

use crate::errors::{Error, Result, SettingsConflict};
use crate::json::{self, Object};
use crate::models::{ModelSelection, Profile, WorkspaceBinding};
use crate::{git, l, paths};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const LOCAL_SETTINGS_PATH: &str = ".claude/settings.local.json";
pub const GIT_EXCLUDE_LINE: &str = "/.claude/settings.local.json";
const GIT_EXCLUDE_COMMENT: &str = "# KeyZapper: projektlokale Claude-Code-Einstellungen";
const SCHEMA_URL: &str = "https://json.schemastore.org/claude-code-settings.json";
pub const AUTH_ENV_KEYS: [&str; 3] = ["ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];
pub const PROVIDER_ENV_KEYS: [&str; 3] = ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"];
pub const MODEL_ENV_KEYS: [&str; 4] =
    ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingHealth {
    Active,
    NotApplied,
    /// Settings keys whose current value differs from what the profile requires.
    Drifted(Vec<String>),
    FolderMissing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingStatus {
    pub health: BindingHealth,
    pub conflicts: Vec<SettingsConflict>,
}

pub struct ApplyResult {
    pub binding: WorkspaceBinding,
    pub changed: bool,
    pub warnings: Vec<SettingsConflict>,
}

#[derive(Clone, Debug)]
pub struct SettingsBinder {
    pub helper_path: String,
    pub managed_settings_paths: Vec<String>,
    pub user_settings_path: String,
}

/// `/Library/Application Support/ClaudeCode` on macOS, `C:\Program Files\ClaudeCode` on Windows,
/// each with `managed-settings.json` and the drop-ins in `managed-settings.d`.
pub fn default_managed_settings_paths() -> Vec<String> {
    #[cfg(windows)]
    let base = PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into())).join("ClaudeCode");
    #[cfg(not(windows))]
    let base = PathBuf::from("/Library/Application Support/ClaudeCode");
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(base.join("managed-settings.d"))
        .map(|dir| dir.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    drop_ins.sort();
    std::iter::once(base.join("managed-settings.json")).chain(drop_ins).map(|p| p.to_string_lossy().into_owned()).collect()
}

pub fn default_user_settings_path() -> String {
    paths::home_dir().join(".claude").join("settings.json").to_string_lossy().into_owned()
}

/// Folder whose local settings Claude Code reads: the main worktree root inside git repositories
/// (Claude Code ≥ 2.1.288, spike T4/T11/T15), otherwise the folder itself.
pub fn settings_root(folder: &str) -> String {
    let path = paths::canonical_path(folder);
    let Some(common) = git::run_path(&["rev-parse", "--path-format=absolute", "--git-common-dir"], &path) else { return path };
    let common = PathBuf::from(common);
    if common.file_name().is_some_and(|n| n == ".git") {
        if let Some(parent) = common.parent() {
            return paths::canonical_path(&parent.to_string_lossy());
        }
    }
    git::run_path(&["rev-parse", "--show-toplevel"], &path).map(|p| paths::canonical_path(&p)).unwrap_or(path)
}

/// Quotes the helper path for the shell Claude Code uses for `apiKeyHelper`: `/bin/sh` on macOS, `cmd` on Windows.
pub fn shell_quote(path: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", path.replace('"', ""))
    } else {
        sh_quote(path)
    }
}

/// Single quotes for `/bin/sh` (macOS, and Claude Code inside WSL).
pub fn sh_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// The helper as the project's Claude Code calls it. A project inside WSL (`\\wsl.localhost\…`) runs Claude Code as a
/// Linux process: it calls the Windows helper through WSL interop under `/bin/sh`, so the helper's path is given as
/// `/mnt/<drive>/…` and quoted for sh. The helper then reads `keys.json` from the Windows user's `%LOCALAPPDATA%` as usual.
pub fn helper_invocation(helper_path: &str, root: &str) -> String {
    if paths::is_wsl_path(root) {
        if let Some(linux) = paths::windows_to_wsl_path(helper_path) {
            return sh_quote(&linux);
        }
    }
    shell_quote(helper_path)
}

/// A project inside WSL can only use a helper on a drive letter (`/mnt/<drive>`); `apply` refuses other installs
/// instead of writing a command that `/bin/sh` in WSL cannot run.
pub fn wsl_helper_conflict(helper_path: &str, root: &str) -> Option<SettingsConflict> {
    if !paths::is_wsl_path(root) || paths::windows_to_wsl_path(helper_path).is_some() {
        return None;
    }
    Some(SettingsConflict::blocking(l!("KeyZapper ist unter %@ installiert; Claude Code in WSL erreicht den Helper nur auf einem Laufwerk mit Buchstaben (/mnt/…).", helper_path)))
}

pub fn is_truthy(value: Option<&str>) -> bool {
    match value.map(str::to_lowercase) {
        Some(v) if !v.is_empty() => !["0", "false", "no", "off"].contains(&v.as_str()),
        _ => false,
    }
}

fn is_directory(path: &str) -> bool {
    Path::new(path).is_dir()
}

fn local_settings_file(root: &str) -> PathBuf {
    Path::new(root).join(".claude").join("settings.local.json")
}

/// True if the file exists but is not a JSON object for Claude Code (strict like `JSON.parse`: no trailing commas).
pub fn is_unparsable_json(path: &str) -> bool {
    match std::fs::read(path) {
        Ok(data) => json::parse_strict(&String::from_utf8_lossy(&data)).is_none(),
        Err(_) => false,
    }
}

/// The settings file leniently parsed; None if missing, error if unreadable.
pub fn read_json(path: &str) -> Result<Option<Object>> {
    if !Path::new(path).exists() {
        return Ok(None);
    }
    let data = std::fs::read(path).map_err(|_| Error::SettingsUnreadable(path.to_string()))?;
    parse(&data).map(Some)
}

fn parse(data: &[u8]) -> Result<Object> {
    json::parse_lenient(&String::from_utf8_lossy(data)).ok_or_else(|| Error::SettingsUnreadable(l!("kein gültiges JSON-Objekt")))
}

/// Serialises settings changes across KeyZapper processes.
pub(crate) fn with_lock<T>(body: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock_path = std::env::temp_dir().join("KeyZapper-settings.lock");
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock_path);
    match file {
        Ok(file) => {
            let _ = file.lock();
            let result = body();
            let _ = file.unlock();
            result
        }
        Err(_) => body(),
    }
}

impl SettingsBinder {
    pub fn new(helper_path: String) -> Self {
        Self { helper_path, managed_settings_paths: default_managed_settings_paths(), user_settings_path: default_user_settings_path() }
    }

    /// The values the app owns for a profile. `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN` are set to "" to
    /// neutralise values inherited from the IDE/shell environment (otherwise headers get mixed, spike T6/T7/T12).
    /// In pool mode the helper is asked every minute, so an exhausted key is replaced quickly.
    /// A `selection` (models of one tenant) replaces the profile's model values; helper and endpoint stay the same.
    /// Subscription profiles have no gateway models, so a selection does not apply to them.
    pub fn desired_values(&self, profile: &Profile, pooled: bool, selection: Option<&ModelSelection>) -> BTreeMap<String, String> {
        self.desired_values_for("", profile, pooled, selection)
    }

    /// Like `desired_values`, with the helper command for the project `root` (see `helper_invocation`).
    pub fn desired_values_for(&self, root: &str, profile: &Profile, pooled: bool, selection: Option<&ModelSelection>) -> BTreeMap<String, String> {
        if profile.is_subscription() {
            return subscription_values(profile);
        }
        let pooled = pooled && !profile.is_sso(); // pooling needs static keys
        let endpoint = profile.endpoint.trim_end_matches('/').to_string();
        let command = if pooled { "pool" } else { "credential" };
        let mut values = BTreeMap::from([
            ("apiKeyHelper".to_string(), format!("{} {command} --profile {}", helper_invocation(&self.helper_path, root), profile.id)),
            ("env.ANTHROPIC_BASE_URL".to_string(), endpoint),
            ("env.ANTHROPIC_API_KEY".to_string(), String::new()),
            ("env.ANTHROPIC_AUTH_TOKEN".to_string(), String::new()),
        ]);
        for (name, value) in profile.model_environment_for(selection) {
            values.insert(format!("env.{name}"), value);
        }
        if pooled {
            values.insert("env.CLAUDE_CODE_API_KEY_HELPER_TTL_MS".into(), "60000".into());
        }
        for (name, value) in profile.environment.iter().flatten() {
            if Profile::is_allowed_environment_name(name) {
                values.insert(format!("env.{name}"), value.clone());
            }
        }
        values
    }

    // MARK: Apply / revert

    /// `selection` is the binding's tenant model set (None = as in the profile); callers re-applying a binding pass
    /// its stored one (`previous.and_then(|p| p.models.as_ref())`) so the choice survives.
    pub fn apply(&self, profile: &Profile, folder: &str, previous: Option<&WorkspaceBinding>, pooled: bool, selection: Option<&ModelSelection>) -> Result<ApplyResult> {
        let pooled = pooled && !profile.is_sso() && !profile.is_subscription();
        let root = paths::canonical_path(folder);
        if !is_directory(&root) {
            return Err(Error::FolderNotFound(root));
        }
        let expected = settings_root(&root);
        if expected != root {
            return Err(Error::NotSettingsRoot { folder: root, root: expected });
        }
        let desired = self.desired_values_for(&root, profile, pooled, selection);
        let previous_values = previous.map(|p| p.managed_values.clone()).unwrap_or_default();
        let mut outer = self.environment_conflicts(&root, &BTreeMap::new());
        outer.extend(wsl_helper_conflict(&self.helper_path, &root));
        let blocking: Vec<_> = outer.iter().filter(|c| c.blocking).cloned().collect();
        if !blocking.is_empty() {
            return Err(Error::SettingsConflicts(blocking));
        }
        let changed = with_lock(|| {
            mutate_settings(&local_settings_file(&root), |settings| {
                let local = local_conflicts(settings, &desired, &previous_values);
                if !local.is_empty() {
                    return Err(Error::SettingsConflicts(local));
                }
                apply_values(settings, &desired, &previous_values);
                Ok(())
            })
        })?;
        let exclude = ensure_git_excluded(&root, LOCAL_SETTINGS_PATH, GIT_EXCLUDE_LINE)?.or_else(|| previous.and_then(|p| p.git_exclude_entry.clone()));
        let binding = WorkspaceBinding {
            id: previous.map(|p| p.id.clone()).unwrap_or_else(crate::models::new_id),
            path: root,
            profile_id: profile.id.clone(),
            managed_values: desired,
            git_exclude_entry: exclude,
            pooled: pooled.then_some(true),
            mcp_server_ids: previous.map(|p| p.mcp_server_ids.clone()).unwrap_or_default(),
            mcp_managed: previous.map(|p| p.mcp_managed.clone()).unwrap_or_default(),
            mcp_git_exclude_entry: previous.and_then(|p| p.mcp_git_exclude_entry.clone()),
            models: selection.filter(|_| !profile.is_subscription()).cloned(),
        };
        Ok(ApplyResult { binding, changed, warnings: outer })
    }

    /// Removes only values that still equal what the app wrote. Returns keys left in place because
    /// they were changed by someone else in the meantime.
    pub fn revert(&self, binding: &WorkspaceBinding) -> Result<Vec<String>> {
        let mut kept = Vec::new();
        if is_directory(&binding.path) {
            with_lock(|| mutate_settings(&local_settings_file(&binding.path), |s| {
                kept = revert_values(s, &binding.managed_values);
                Ok(())
            }))?;
            if let Some(line) = &binding.git_exclude_entry {
                remove_git_exclude(&binding.path, line)?;
            }
        }
        kept.sort();
        Ok(kept)
    }

    // MARK: Global default profile (~/.claude/settings.json)

    /// Writes the profile into the user settings so folders without their own assignment use it too. Existing
    /// values for the same keys (e.g. a plaintext key) are replaced on purpose; `backup_user_settings()` keeps them.
    pub fn apply_global(&self, profile: &Profile, previous: Option<&WorkspaceBinding>) -> Result<ApplyResult> {
        let managed = self.managed_conflicts();
        let blocking: Vec<_> = managed.iter().filter(|c| c.blocking).cloned().collect();
        if !blocking.is_empty() {
            return Err(Error::SettingsConflicts(blocking));
        }
        let desired = self.desired_values(profile, false, None);
        let previous_values = previous.map(|p| p.managed_values.clone()).unwrap_or_default();
        let changed = with_lock(|| {
            mutate_settings(Path::new(&self.user_settings_path), |settings| {
                if !settings.contains_key("$schema") {
                    settings.insert("$schema".into(), SCHEMA_URL.into());
                }
                apply_values(settings, &desired, &previous_values);
                Ok(())
            })
        })?;
        let mut binding = WorkspaceBinding::new(self.user_settings_path.clone(), profile.id.clone());
        if let Some(previous) = previous {
            binding.id = previous.id.clone();
        }
        binding.managed_values = desired;
        Ok(ApplyResult { binding, changed, warnings: managed })
    }

    /// Removes the default profile's values from the user settings (only those still unchanged).
    pub fn revert_global(&self, binding: &WorkspaceBinding) -> Result<Vec<String>> {
        let mut kept = Vec::new();
        with_lock(|| mutate_settings(Path::new(&self.user_settings_path), |s| {
            kept = revert_values(s, &binding.managed_values);
            Ok(())
        }))?;
        kept.sort();
        Ok(kept)
    }

    pub fn inspect_global(&self, binding: &WorkspaceBinding, profile: &Profile) -> BindingHealth {
        if is_unparsable_json(&self.user_settings_path) {
            return BindingHealth::NotApplied;
        }
        let Ok(Some(settings)) = read_json(&self.user_settings_path) else { return BindingHealth::NotApplied };
        let _ = binding;
        health(&settings, &self.desired_values(profile, false, None))
    }

    /// Copies the user settings to `settings.json.keyzapper-<timestamp>.bak` before KeyZapper changes them.
    pub fn backup_user_settings(&self) -> Result<Option<PathBuf>> {
        if !Path::new(&self.user_settings_path).exists() {
            return Ok(None);
        }
        let target = PathBuf::from(format!("{}.keyzapper-{}.bak", self.user_settings_path, crate::time::local_stamp()));
        let _ = std::fs::remove_file(&target);
        std::fs::copy(&self.user_settings_path, &target)?;
        Ok(Some(target))
    }

    /// Makes `~/.claude/settings.json` valid for Claude Code: keeps everything readable, removes plaintext keys,
    /// turns non-text `env` values into text and adds `$schema`. An unreadable file is replaced by a fresh one.
    /// The original is backed up first; returns the backup.
    pub fn repair_user_settings(&self) -> Result<Option<PathBuf>> {
        let backup = self.backup_user_settings()?;
        let mut settings = read_json(&self.user_settings_path).ok().flatten().unwrap_or_default();
        if !settings.contains_key("$schema") {
            settings.insert("$schema".into(), SCHEMA_URL.into());
        }
        match settings.get("env").cloned() {
            Some(serde_json::Value::Object(mut env)) => {
                for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
                    if env.get(key).and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()) {
                        env.remove(key);
                    }
                }
                for value in env.values_mut() {
                    if !value.is_string() {
                        *value = serde_json::Value::String(display_value(value));
                    }
                }
                if env.is_empty() {
                    settings.remove("env");
                } else {
                    settings.insert("env".into(), serde_json::Value::Object(env));
                }
            }
            Some(_) => {
                settings.remove("env");
            }
            None => {}
        }
        let text = json::to_pretty_sorted(&serde_json::Value::Object(settings));
        paths::write_atomic(Path::new(&self.user_settings_path), text.as_bytes(), false)?;
        Ok(backup)
    }

    // MARK: Status

    pub fn inspect(&self, binding: &WorkspaceBinding, profile: &Profile, global_values: &BTreeMap<String, String>) -> BindingStatus {
        if !is_directory(&binding.path) {
            return BindingStatus { health: BindingHealth::FolderMissing, conflicts: vec![] };
        }
        let mut conflicts = self.environment_conflicts(&binding.path, global_values);
        let local = local_settings_file(&binding.path).to_string_lossy().into_owned();
        // Strict check first: Claude Code ignores a file the lenient reader would still accept (e.g. trailing commas).
        let settings = match (is_unparsable_json(&local), read_json(&local)) {
            (false, Ok(settings)) => settings.unwrap_or_default(),
            _ => {
                conflicts.push(SettingsConflict::blocking(l!("%@ ist kein gültiges JSON.", LOCAL_SETTINGS_PATH)));
                return BindingStatus { health: BindingHealth::Drifted(vec![]), conflicts };
            }
        };
        let desired = self.desired_values_for(&binding.path, profile, binding.is_pooled(), binding.models.as_ref());
        BindingStatus { health: health(&settings, &desired), conflicts }
    }

    // MARK: Conflicts

    /// Managed settings, git tracking, provider switches (blocking) and lower-precedence layers (warnings).
    pub fn environment_conflicts(&self, root: &str, global_values: &BTreeMap<String, String>) -> Vec<SettingsConflict> {
        let mut result = self.managed_conflicts();
        if git::status(&["ls-files", "--error-unmatch", LOCAL_SETTINGS_PATH], root) == 0 {
            result.push(SettingsConflict::blocking(l!("%@ ist im Git-Repository eingecheckt und würde geteilt.", LOCAL_SETTINGS_PATH)));
        }
        let project = Path::new(root).join(".claude").join("settings.json").to_string_lossy().into_owned();
        let local = local_settings_file(root).to_string_lossy().into_owned();
        let mut layers = vec![
            (l!("Benutzereinstellungen"), self.user_settings_path.clone()),
            (l!("Projekteinstellungen"), project.clone()),
            (l!("Lokale Projekteinstellungen"), local.clone()),
        ];
        layers.extend(self.managed_settings_paths.iter().map(|p| (l!("Verwaltete Firmeneinstellung"), p.clone())));
        for (name, path) in layers {
            // User settings get one app-wide banner; invalid local settings are reported by inspect().
            if path != self.user_settings_path && path != local && is_unparsable_json(&path) {
                result.push(SettingsConflict::warning(l!("%@ (%@) sind kein gültiges JSON; Claude Code ignoriert die Datei.", name, path)));
                continue;
            }
            let Ok(Some(settings)) = read_json(&path) else { continue };
            for key in PROVIDER_ENV_KEYS {
                if is_truthy(json::get_str(&settings, &format!("env.{key}"))) {
                    result.push(SettingsConflict::blocking(l!("%@ (%@): „%@“ leitet Claude Code an LiteLLM vorbei.", name, path, key)));
                }
            }
            if path == self.user_settings_path || path == project {
                // Values of KeyZapper's own default profile in the user settings are expected, not a conflict.
                let keys = std::iter::once("apiKeyHelper".to_string()).chain(AUTH_ENV_KEYS.iter().map(|k| format!("env.{k}")));
                for key in keys {
                    let own = path == self.user_settings_path
                        && global_values.get(&key).is_some_and(|v| json::get_str(&settings, &key) == Some(v.as_str()));
                    if json::has(&settings, &key) && !own {
                        result.push(SettingsConflict::warning(l!("%@ (%@) setzen „%@“; die Projektzuordnung hat Vorrang.", name, path, key)));
                    }
                }
            }
        }
        result
    }

    /// Managed (company) settings: auth keys are blocking, model variables override the profile's models.
    pub fn managed_conflicts(&self) -> Vec<SettingsConflict> {
        let mut result = Vec::new();
        for path in &self.managed_settings_paths {
            let Ok(Some(managed)) = read_json(path) else { continue };
            for key in std::iter::once("apiKeyHelper".to_string()).chain(AUTH_ENV_KEYS.iter().map(|k| format!("env.{k}"))) {
                if json::has(&managed, &key) {
                    result.push(SettingsConflict::blocking(l!(
                        "Verwaltete Firmeneinstellung %@ setzt „%@“ und hat Vorrang vor der Projektzuordnung.", path, key
                    )));
                }
            }
            for key in MODEL_ENV_KEYS.iter().map(|k| format!("env.{k}")) {
                if json::has(&managed, &key) {
                    result.push(SettingsConflict::warning(l!(
                        "Verwaltete Firmeneinstellung %@ setzt „%@“ und überstimmt die Modelle des Profils.", path, key
                    )));
                }
            }
        }
        result
    }
}

/// Claude Code's own claude.ai login: helper, gateway, keys and gateway model names from other layers (user settings,
/// IDE/shell environment) are set to "" so they do not apply; Claude Code then falls back to the subscription.
fn subscription_values(profile: &Profile) -> BTreeMap<String, String> {
    let mut values: BTreeMap<String, String> = std::iter::once("apiKeyHelper".to_string())
        .chain(AUTH_ENV_KEYS.iter().chain(MODEL_ENV_KEYS.iter()).map(|k| format!("env.{k}")))
        .map(|key| (key, String::new()))
        .collect();
    for (name, value) in profile.model_environment() {
        values.insert(format!("env.{name}"), value);
    }
    for (name, value) in profile.environment.iter().flatten() {
        if Profile::is_allowed_environment_name(name) {
            values.insert(format!("env.{name}"), value.clone());
        }
    }
    values
}

fn health(settings: &Object, desired: &BTreeMap<String, String>) -> BindingHealth {
    let differing: Vec<String> =
        desired.iter().filter(|(k, v)| json::get_str(settings, k) != Some(v.as_str())).map(|(k, _)| k.clone()).collect();
    if differing.is_empty() {
        BindingHealth::Active
    } else if desired.keys().all(|k| !json::has(settings, k)) {
        BindingHealth::NotApplied
    } else {
        BindingHealth::Drifted(differing)
    }
}

fn apply_values(settings: &mut Object, desired: &BTreeMap<String, String>, previous: &BTreeMap<String, String>) {
    for (key, value) in desired {
        json::set(settings, key, value);
    }
    for (key, value) in previous {
        if !desired.contains_key(key) && json::get_str(settings, key) == Some(value.as_str()) {
            json::remove(settings, key);
        }
    }
}

fn revert_values(settings: &mut Object, managed: &BTreeMap<String, String>) -> Vec<String> {
    let mut kept = Vec::new();
    for (key, value) in managed {
        if json::get_str(settings, key) == Some(value.as_str()) {
            json::remove(settings, key);
        } else if json::has(settings, key) {
            kept.push(key.clone());
        }
    }
    kept
}

/// `true` → "1" like the macOS app (NSNumber), numbers as written, objects and arrays as JSON.
fn display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Bool(b) => if *b { "1" } else { "0" }.into(),
        serde_json::Value::Null => "<null>".into(),
        other => other.to_string(),
    }
}

/// Conflicts in the local file itself. Values are never included in messages (they may be secrets).
fn local_conflicts(settings: &Object, desired: &BTreeMap<String, String>, previous: &BTreeMap<String, String>) -> Vec<SettingsConflict> {
    desired
        .iter()
        .filter_map(|(key, wanted)| {
            if !json::has(settings, key) {
                return None;
            }
            let current = json::get_str(settings, key);
            if current == Some(wanted.as_str()) || (current.is_some() && current == previous.get(key).map(String::as_str)) {
                return None;
            }
            Some(SettingsConflict::blocking(l!("%@: „%@“ ist bereits mit einem anderen Wert gesetzt. Bitte manuell entfernen.", LOCAL_SETTINGS_PATH, key)))
        })
        .collect()
}

/// Read-modify-write with optimistic concurrency: the file is replaced atomically (rename) only if it
/// is still byte-identical to what was read. Returns false when nothing had to change (idempotent).
pub(crate) fn mutate_settings(file: &Path, transform: impl FnMut(&mut Object) -> Result<()>) -> Result<bool> {
    let mut transform = transform;
    let path = file.to_string_lossy().into_owned();
    let read = || -> Result<Option<Vec<u8>>> {
        if !file.exists() {
            return Ok(None);
        }
        std::fs::read(file).map(Some).map_err(|_| Error::SettingsUnreadable(path.clone()))
    };
    for _ in 0..3 {
        let original = read()?;
        let mut settings = match &original {
            Some(data) => parse(data)?,
            None => Object::new(),
        };
        let before = settings.clone();
        transform(&mut settings)?;
        if settings.get("env").and_then(|e| e.as_object()).is_some_and(|e| e.is_empty()) {
            settings.remove("env");
        }
        // Rewrite even without changes if Claude Code cannot parse the file as it is (e.g. trailing commas).
        if settings == before && (original.is_none() || !is_unparsable_json(&path)) {
            return Ok(false);
        }
        if settings.is_empty() {
            if read().ok().flatten() != original {
                continue;
            }
            let _ = std::fs::remove_file(file);
            if let Some(dir) = file.parent().filter(|d| d.file_name().is_some_and(|n| n == ".claude")) {
                if std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
                    let _ = std::fs::remove_dir(dir);
                }
            }
            return Ok(true);
        }
        let dir = file.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let text = json::to_pretty_sorted(&serde_json::Value::Object(settings));
        let tmp = dir.join(format!(".{}.keyzapper-{}", file.file_name().unwrap_or_default().to_string_lossy(), uuid::Uuid::new_v4()));
        std::fs::write(&tmp, text.as_bytes())?;
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(file) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        if read().ok().flatten() != original {
            let _ = std::fs::remove_file(&tmp);
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp, file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::SettingsUnreadable(format!("{path}: {e}")));
        }
        return Ok(true);
    }
    Err(Error::ConcurrentModification(path))
}

// MARK: Git exclude

fn exclude_file(root: &str) -> Option<PathBuf> {
    git::run_path(&["rev-parse", "--path-format=absolute", "--git-common-dir"], root).map(|c| PathBuf::from(c).join("info").join("exclude"))
}

/// Ensures `relative` (e.g. the local settings file) is git-ignored via `<git-common-dir>/info/exclude` (shared
/// by all worktrees). Returns the line added, or None if nothing was added.
pub(crate) fn ensure_git_excluded(root: &str, relative: &str, line: &str) -> Result<Option<String>> {
    let Some(exclude) = exclude_file(root) else { return Ok(None) };
    if git::status(&["check-ignore", "-q", relative], root) == 0 {
        return Ok(None);
    }
    if let Some(dir) = exclude.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("{GIT_EXCLUDE_COMMENT}\n{line}\n"));
    paths::write_atomic(&exclude, text.as_bytes(), false)?;
    Ok(Some(line.to_string()))
}

pub(crate) fn remove_git_exclude(root: &str, line: &str) -> Result<()> {
    let Some(exclude) = exclude_file(root) else { return Ok(()) };
    let Ok(text) = std::fs::read_to_string(&exclude) else { return Ok(()) };
    let mut lines: Vec<&str> = text.split('\n').collect();
    let Some(index) = lines.iter().rposition(|l| *l == line) else { return Ok(()) };
    lines.remove(index);
    if index > 0 && lines[index - 1] == GIT_EXCLUDE_COMMENT {
        lines.remove(index - 1);
    }
    paths::write_atomic(&exclude, lines.join("\n").as_bytes(), false)?;
    Ok(())
}
