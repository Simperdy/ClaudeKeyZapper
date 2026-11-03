//! App state and actions, ported from the macOS app's `AppModel`. Keys live in the key file and are read
//! directly (no keychain, no prompts). Slow work (git status, gateway, updates) runs in `commands`.

use keyzapper_core::audit::Finding;
use keyzapper_core::backup::BackupPayload;
use keyzapper_core::backup_store::BackupStore;
use keyzapper_core::claude_desktop::{BearerTokenType, CredentialKind, DesktopSso};
use keyzapper_core::claude_office::{self, office_profile, plan_retry, plan_sync, DesiredOffice, OfficeClient, OfficeStatus, Outcome, Sideload, SyncAction, SyncJob};
use keyzapper_core::gateway::KeyBudget;
use keyzapper_core::keys::{masked_hint, KeyStore};
use keyzapper_core::managed::{host_of, is_secure_endpoint, ManagedConfig};
use keyzapper_core::metadata::MetadataStore;
use keyzapper_core::models::{
    new_id, parse_id, selection_for_tenant, tenants_of, AppState, AuthType, McpServer, McpTransport, ModelSelection, OidcTokenType, Profile,
    WorkspaceBinding,
};
use keyzapper_core::paths::{self, abbreviate_home, folder_name};
use keyzapper_core::settings::{self, BindingHealth, BindingStatus, SettingsBinder};
use keyzapper_core::update::ReleaseInfo;
use keyzapper_core::tokens::TokenStore;
use keyzapper_core::mcp_catalog::{Catalog, FetchError};
use keyzapper_core::tools::{Tool, ToolStatus};
use keyzapper_core::{agents, claude_desktop, cli, helper, l, legacy_keychain, mcp, mcp_catalog, oidc, subscription, update, whats_new};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A budget the gateway reported: for a tenant of a multi-tenant gateway, or None for the profile itself.
#[derive(Clone, Debug, PartialEq)]
pub struct TenantBudget {
    pub tenant: Option<String>,
    pub budget: KeyBudget,
}

pub struct Model {
    pub state: AppState,
    /// Per profile the budgets of `budget_targets`: one for a key, one per tenant of an SSO profile's projects.
    pub budgets: HashMap<String, Vec<TenantBudget>>,
    /// Model IDs the gateway lists per profile (`/v1/models`), loaded with the budgets. Their `<tenant>/` prefixes are the
    /// tenants a project can choose; kept when a reload fails.
    pub gateway_models: HashMap<String, Vec<String>>,
    pub available_update: Option<ReleaseInfo>,
    pub installing_update: bool,
    /// Highlights of the releases since the version last seen, shown once after an update.
    pub whats_new: Vec<whats_new::Release>,
    /// Findings of the logical check of `~/.claude/settings.json`.
    pub global_findings: Vec<Finding>,
    /// State of the default profile in `~/.claude/settings.json`, None if none is set.
    pub global_health: Option<BindingHealth>,
    pub statuses: HashMap<String, BindingStatus>,
    /// Per binding: whether its `.mcp.json` entries are still as written.
    pub mcp_active: HashMap<String, bool>,
    pub outdated_clis: Vec<String>,
    /// Unrestored OneDrive backup found on a fresh install; while set, the backup is not overwritten.
    pub available_backup: Option<AppState>,
    pub backup_problem: Option<String>,
    /// Profiles whose key is still only in the macOS keychain (KeyZapper ≤ 1.x).
    pub keychain_profiles: Vec<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Why Claude in Office cannot be set up right now (unusable settings, gateway not allowed, no template); nothing was
    /// written then. Recomputed by every sync run.
    office_problem: Option<String>,
    /// Where Office keeps its manifests on this machine (tests point it into a temporary folder).
    pub office_sideload: Sideload,
    /// The last good copy of Anthropic's manifest template.
    office_template: PathBuf,
    /// Claude Desktop's configuration library (tests point it into a temporary folder).
    pub desktop_library: PathBuf,
    /// Why the Claude Desktop setup IT asks for (`AutoApply`) is not written; recomputed by every run.
    desktop_problem: Option<String>,
    pub config: ManagedConfig,
    /// None for development builds.
    pub app_version: Option<String>,
    pub keys: KeyStore,
    pub tokens: TokenStore,
    /// Claude Code's claude.ai login with plan and usage limits; None until checked or without subscription profile.
    pub subscription: Option<subscription::Status>,
    /// Start of the last budget and subscription refresh, so opening the tray popover only reloads stale values.
    pub usage_refreshed_at: Option<std::time::Instant>,
    /// SSO profiles whose refresh failed (session expired); cleared on login/logout.
    pub sso_expired: HashSet<String>,
    /// SSO profiles with a browser login in progress.
    pub sso_logging_in: HashSet<String>,
    pub binder: Option<SettingsBinder>,
    pub refresh_generation: u64,
    /// True if the window got the system's translucent material (macOS vibrancy, Windows 11 Mica).
    pub vibrancy: bool,
    /// Central MCP server list (`ManagedMcpServersURL`) as last fetched; its servers are managed by IT.
    pub mcp_catalog: Option<Catalog>,
    pub mcp_catalog_problem: Option<String>,
    pub mcp_catalog_loading: bool,
    /// Installed Claude tools; None until checked.
    pub tools: Option<Vec<ToolStatus>>,
    pub installing_tool: Option<Tool>,
    /// KeyZapper's entry in Claude Desktop's configuration library.
    pub claude_desktop: claude_desktop::Status,
    backup_dir: Option<PathBuf>,
    metadata: MetadataStore,
    /// Set when metadata could not be read (e.g. newer schema); saving is blocked to avoid data loss.
    metadata_unreadable: bool,
}

/// Editor input for a new or changed profile.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    pub id: Option<String>,
    pub name: String,
    pub endpoint: String,
    pub model_alias: String,
    pub opus_model: String,
    pub sonnet_model: String,
    pub haiku_model: String,
    pub environment_text: String,
    pub key: String,
    pub auth_type: AuthType,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_scope: String,
    pub oidc_token_type: OidcTokenType,
    /// Claude in Office of the profile; None leaves it as it is (the editor sends none for subscription profiles).
    #[serde(default)]
    pub office: Option<OfficeInput>,
    /// Claude Desktop's own sign-in for the profile; None leaves it as it is (the editor sends none for subscriptions).
    #[serde(default)]
    pub claude_desktop: Option<DesktopSsoInput>,
}

/// Editor input for Claude Desktop's own sign-in with the gateway's OIDC client.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSsoInput {
    pub enabled: bool,
    pub gateway_url: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub scopes: String,
    pub bearer_token_type: BearerTokenType,
    pub credential_kind: CredentialKind,
}

impl DesktopSsoInput {
    /// The profile's block; None when the switch is off (Claude Desktop then uses the helper).
    fn into_sso(self) -> Option<DesktopSso> {
        self.enabled.then(|| DesktopSso {
            gateway_url: self.gateway_url.trim().to_string(),
            oidc_issuer: self.oidc_issuer.trim().to_string(),
            oidc_client_id: self.oidc_client_id.trim().to_string(),
            scopes: self.scopes.trim().to_string(),
            bearer_token_type: self.bearer_token_type,
            credential_kind: self.credential_kind,
            ..DesktopSso::default()
        })
    }
}

/// Editor input for the Claude in Office part of a profile.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeInput {
    pub enabled: bool,
    pub addin_id: String,
    pub gateway_url: String,
    pub entra_client_id: String,
    pub entra_scope: String,
    /// Separated by commas, spaces or line breaks.
    pub disabled_features: String,
}

impl OfficeInput {
    /// The profile's Office settings; None when the switch is off and nothing is filled in.
    fn into_client(self) -> Option<OfficeClient> {
        let client = OfficeClient {
            enabled: self.enabled,
            addin_id: self.addin_id.trim().to_string(),
            gateway_url: self.gateway_url.trim().to_string(),
            entra_client_id: self.entra_client_id.trim().to_string(),
            entra_scope: self.entra_scope.trim().to_string(),
            disabled_features: self.disabled_features.split([',', ';', ' ', '\t', '\r', '\n']).filter(|f| !f.is_empty()).map(String::from).collect(),
        };
        let blank = client.addin_id.is_empty()
            && client.gateway_url.is_empty()
            && client.entra_client_id.is_empty()
            && client.entra_scope.is_empty()
            && client.disabled_features.is_empty();
        (client.enabled || !blank).then_some(client)
    }
}

/// Editor input for a new or changed MCP server.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub id: Option<String>,
    pub name: String,
    pub transport: McpTransport,
    pub command: String,
    /// One argument per line.
    pub args_text: String,
    /// `NAME=value` lines.
    pub env_text: String,
    pub url: String,
    /// `Name: value` lines.
    pub headers_text: String,
    #[serde(default)]
    pub oauth_client_id: String,
    #[serde(default)]
    pub oauth_callback_port: String,
    #[serde(default)]
    pub oauth_scopes: String,
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// https URL with a host (http only for loopback), as typed (trimmed).
pub fn valid_endpoint(text: &str) -> Option<String> {
    let trimmed = text.trim();
    is_secure_endpoint(trimmed).then(|| trimmed.to_string())
}

impl Model {
    pub fn load(version: Option<String>) -> Self {
        Self::load_from(version, ManagedConfig::load(), paths::data_dir())
    }

    /// Loads from `data_dir` with the managed settings `config` as given. Tests pass an empty config and a temporary folder,
    /// so they neither read the real managed preferences nor write the real data or backup folder.
    fn load_from(version: Option<String>, config: ManagedConfig, data_dir: PathBuf) -> Self {
        keyzapper_core::proxy::configure(config.proxy_url.clone(), config.ignore_download_certificate_errors);
        let backup_dir = BackupStore::directory(&config);
        let metadata = MetadataStore::new(data_dir.join("state.json"), true);
        let mut model = Model {
            state: AppState::default(),
            budgets: HashMap::new(),
            gateway_models: HashMap::new(),
            available_update: None,
            installing_update: false,
            whats_new: vec![],
            global_findings: vec![],
            global_health: None,
            statuses: HashMap::new(),
            mcp_active: HashMap::new(),
            outdated_clis: vec![],
            available_backup: None,
            backup_problem: None,
            keychain_profiles: vec![],
            error: None,
            notice: None,
            office_problem: None,
            office_sideload: Sideload::system(),
            office_template: claude_office::template_file_in(&data_dir),
            desktop_library: claude_desktop::library_dir(),
            desktop_problem: None,
            config,
            app_version: version,
            keys: KeyStore { file: data_dir.join("keys.json") },
            tokens: TokenStore { file: data_dir.join("sso-tokens.json") },
            subscription: None,
            usage_refreshed_at: None,
            sso_expired: HashSet::new(),
            sso_logging_in: HashSet::new(),
            binder: None,
            refresh_generation: 0,
            vibrancy: false,
            mcp_catalog: None,
            mcp_catalog_problem: None,
            mcp_catalog_loading: false,
            tools: None,
            installing_tool: None,
            claude_desktop: claude_desktop::Status::default(),
            backup_dir,
            metadata,
            metadata_unreadable: false,
        };
        match model.metadata.load() {
            Ok(state) => model.state = state,
            Err(e) => {
                model.metadata_unreadable = true;
                model.error = Some(e.to_string());
            }
        }
        model.load_whats_new();
        if model.config.one_drive_backup || model.config.backup_directory.is_some() {
            match &model.backup_dir {
                Some(dir) => {
                    if model.state.profiles.is_empty() {
                        if let Ok(Some(backup)) = BackupStore::new(dir.clone()).read() {
                            if !backup.profiles.is_empty() {
                                model.available_backup = Some(backup);
                            }
                        }
                    }
                }
                None => {
                    model.backup_problem =
                        Some(l!("OneDrive-Backup ist aktiviert, aber es wurde kein OneDrive-Ordner gefunden. Bitte in OneDrive anmelden."))
                }
            }
        }
        match helper_path() {
            Some(path) => model.binder = Some(SettingsBinder::new(path.to_string_lossy().into_owned())),
            None => model.error = Some(l!("Hilfsprogramm %@ wurde nicht gefunden. Bitte App neu installieren.", helper::EXECUTABLE_NAME)),
        }
        model.sync_managed_profiles();
        model.migrate_helper_path();
        if let Some(url) = model.config.managed_mcp_servers_url.clone() {
            model.mcp_catalog = mcp_catalog::load_cache(&url);
            model.sync_managed_mcp_servers(None);
        }
        model
    }

    /// After an update: the highlights since the version last seen. Existing users updating from a release before this
    /// dialog have no seen version; a fresh install (no profiles yet) shows nothing.
    fn load_whats_new(&mut self) {
        let Some(current) = self.app_version.clone() else { return };
        let seen = whats_new::seen_version();
        self.whats_new = match &seen {
            Some(seen) if update::is_newer(&current, seen) => whats_new::since(Some(seen), &current),
            None if !self.state.profiles.is_empty() => whats_new::since(None, &current),
            _ => vec![],
        };
        if self.whats_new.is_empty() && seen.as_deref() != Some(current.as_str()) {
            let _ = whats_new::mark_seen(&current);
        }
    }

    pub fn dismiss_whats_new(&mut self) {
        if let Some(current) = &self.app_version {
            let _ = whats_new::mark_seen(current);
        }
        self.whats_new.clear();
    }

    pub fn is_disabled(&self) -> bool {
        self.state.is_disabled()
    }

    /// Update checks include beta releases: as switched in the app, else while a beta is installed (testers get the next).
    pub fn beta_updates(&self) -> bool {
        self.state.beta_updates.unwrap_or_else(|| self.app_version.as_deref().is_some_and(update::is_prerelease))
    }

    /// Switches beta releases for update checks on or off; switched off, an offered beta is no longer shown.
    pub fn set_beta_updates(&mut self, enabled: bool) {
        self.state.beta_updates = Some(enabled);
        if !enabled {
            self.available_update = self.available_update.take().filter(|release| !release.prerelease);
        }
        self.persist();
    }

    pub fn minimum_cli_version(&self) -> String {
        self.config.minimum_claude_code_version.clone().unwrap_or_else(|| cli::MINIMUM_TESTED_VERSION.into())
    }

    fn profile(&self, id: &str) -> Option<Profile> {
        self.state.profile(id).cloned()
    }

    fn binding(&self, id: &str) -> Option<WorkspaceBinding> {
        self.state.bindings.iter().find(|b| b.id == id).cloned()
    }

    /// Subscription profiles send nothing to a gateway, so the gateway allowlist does not apply to them.
    fn is_allowed(&self, profile: &Profile) -> bool {
        profile.is_subscription() || self.config.is_endpoint_allowed(&profile.endpoint)
    }

    pub fn has_subscription_profile(&self) -> bool {
        self.state.profiles.iter().any(Profile::is_subscription)
    }

    fn not_allowed_message(&self, endpoint: &str) -> String {
        if !is_secure_endpoint(endpoint) {
            return l!("Der Endpunkt %@ verwendet kein https. Keys werden nur verschlüsselt übertragen (http nur für localhost).", endpoint);
        }
        l!(
            "Der Endpunkt %@ ist laut Firmenrichtlinie nicht freigegeben. Erlaubt: %@",
            host_of(endpoint).unwrap_or_default(),
            self.config.allowed_gateway_hosts.join(", ")
        )
    }

    /// Creates/updates the profiles IT defines via `ManagedProfiles`; developers only add their key.
    fn sync_managed_profiles(&mut self) {
        for managed in self.config.profiles.clone() {
            let mut profile = self.profile(&managed.id).unwrap_or_else(|| {
                Profile::new(managed.id.clone(), managed.name.clone(), managed.endpoint.clone(), managed.model_alias.clone())
            });
            profile.name = managed.name.clone();
            profile.endpoint = managed.endpoint.clone();
            profile.model_alias = managed.model_alias.clone();
            profile.opus_model = managed.opus_model.clone();
            profile.sonnet_model = managed.sonnet_model.clone();
            profile.haiku_model = managed.haiku_model.clone();
            profile.environment = managed.environment.clone();
            profile.auth_type = managed.auth_type;
            profile.oidc_issuer = managed.oidc_issuer.clone();
            profile.oidc_client_id = managed.oidc_client_id.clone();
            profile.oidc_scope = managed.oidc_scope.clone();
            profile.oidc_token_type = managed.oidc_token_type;
            // Claude in Office and Claude Desktop's sign-in are IT's where it sets them; otherwise the user's own block
            // stays, so a managed profile can be tried with them (a block IT drops stays as the user's own until it is
            // switched off).
            if managed.office.is_some() {
                profile.office = managed.office.clone();
            }
            if managed.claude_desktop.is_some() {
                profile.claude_desktop = managed.claude_desktop.clone();
            }
            if self.state.profile(&managed.id) != Some(&profile) {
                self.save_profile(profile, None);
            }
        }
    }

    /// Settings written by KeyZapper 1.x (or an install in another place) call a helper that may no longer exist.
    /// Rewrites them for the bundled helper, quietly.
    fn migrate_helper_path(&mut self) {
        let Some(binder) = self.binder.clone() else { return };
        if self.is_disabled() {
            return;
        }
        let notice = self.notice.clone();
        for binding in self.state.bindings.clone() {
            let Some(profile) = self.profile(&binding.profile_id) else { continue };
            let wanted = binder.desired_values_for(&binding.path, &profile, binding.is_pooled(), binding.models.as_ref());
            if binding.managed_values.get("apiKeyHelper") != wanted.get("apiKeyHelper") && Path::new(&binding.path).is_dir() {
                self.apply(&profile, &binding.path, Some(&binding), None);
            }
        }
        if let Some(global) = self.state.global_binding.clone() {
            if let Some(profile) = self.profile(&global.profile_id) {
                if global.managed_values.get("apiKeyHelper") != binder.desired_values(&profile, false, None).get("apiKeyHelper") {
                    self.apply_global(&profile, Some(&global));
                }
            }
        }
        self.notice = notice;
    }

    // MARK: Profiles

    pub fn save_profile_input(&mut self, input: ProfileInput) -> bool {
        let existing = input.id.as_deref().and_then(parse_id).and_then(|id| self.profile(&id));
        let auth_type = existing.as_ref().map_or(input.auth_type, |p| p.auth_type);
        let (sso, subscription) = (auth_type == AuthType::Oidc, auth_type == AuthType::Subscription);
        let endpoint = match valid_endpoint(&input.endpoint) {
            _ if subscription => String::new(),
            Some(endpoint) => endpoint,
            None => {
                self.error = Some(l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."));
                return false;
            }
        };
        let (environment, invalid) = Profile::parse_environment(&input.environment_text);
        if input.name.trim().is_empty() || !invalid.is_empty() {
            return false;
        }
        // `Some(None)`: Office removed from the profile. `None`: an editor that sent nothing leaves it as it is.
        let office: Option<Option<OfficeClient>> = if subscription { Some(None) } else { input.office.map(OfficeInput::into_client) };
        // Of a managed profile only Claude in Office and Claude Desktop's sign-in can be changed, and only where IT sets neither.
        let managed = existing.as_ref().and_then(|p| self.config.profiles.iter().find(|m| m.id == p.id));
        let office_locked = managed.is_some_and(|m| m.office.is_some());
        let desktop_locked = managed.is_some_and(|m| m.claude_desktop.is_some());
        // Office settings that are switched on must be usable; switched off they may stay half filled.
        if let Some(Some(office)) = office.as_ref().filter(|_| !office_locked) {
            if let Some(message) = office.enabled.then(|| self.office_blocker(office)).flatten() {
                self.error = Some(message);
                return false;
            }
        }
        let desktop: Option<Option<DesktopSso>> = if subscription { Some(None) } else { input.claude_desktop.map(DesktopSsoInput::into_sso) };
        if let Some(Some(desktop)) = desktop.as_ref().filter(|_| !desktop_locked) {
            let mut problems = desktop.problems();
            if problems.is_empty() && !desktop.gateway_url.is_empty() && !self.config.is_endpoint_allowed(&desktop.gateway_url) {
                problems.push(self.not_allowed_message(&desktop.gateway_url));
            }
            if !problems.is_empty() {
                self.error = Some(problems.join(" "));
                return false;
            }
        }
        if sso {
            if let Err(e) = oidc::validate_url(&input.oidc_issuer) {
                self.error = Some(e.to_string());
                return false;
            }
            if input.oidc_client_id.trim().is_empty() {
                return false;
            }
        } else if !subscription && existing.is_none() && input.key.trim().is_empty() {
            return false;
        }
        let mut profile = existing.unwrap_or_else(|| Profile::new(new_id(), String::new(), String::new(), String::new()));
        if !self.config.is_managed(&profile.id) {
            profile.name = input.name.trim().to_string();
            profile.endpoint = endpoint;
            profile.model_alias = input.model_alias.trim().to_string();
            // A subscription uses Claude's own model names; only the default model can be chosen.
            profile.opus_model = non_empty(&input.opus_model).filter(|_| !subscription);
            profile.sonnet_model = non_empty(&input.sonnet_model).filter(|_| !subscription);
            profile.haiku_model = non_empty(&input.haiku_model).filter(|_| !subscription);
            profile.environment = (!environment.is_empty()).then_some(environment);
            if subscription {
                profile.auth_type = AuthType::Subscription;
            }
            if sso {
                profile.auth_type = AuthType::Oidc;
                profile.oidc_issuer = Some(input.oidc_issuer.trim().to_string());
                profile.oidc_client_id = Some(input.oidc_client_id.trim().to_string());
                profile.oidc_scope = non_empty(&input.oidc_scope);
                profile.oidc_token_type = input.oidc_token_type;
            }
        }
        if let Some(office) = office.filter(|_| !office_locked) {
            profile.office = office;
        }
        if let Some(desktop) = desktop.filter(|_| !desktop_locked) {
            profile.claude_desktop = desktop;
        }
        self.save_profile(profile, (!sso && !subscription).then_some(input.key))
    }

    pub fn save_profile(&mut self, profile: Profile, new_key: Option<String>) -> bool {
        if !self.is_allowed(&profile) {
            self.error = Some(self.not_allowed_message(&profile.endpoint));
            return false;
        }
        let old = self.profile(&profile.id);
        if profile.is_sso() && old.as_ref().is_some_and(|o| o.oidc_issuer != profile.oidc_issuer || o.oidc_client_id != profile.oidc_client_id) {
            self.sso_logout(&profile.id);
        }
        match self.state.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile.clone(),
            None => self.state.profiles.push(profile.clone()),
        }
        // The cached model list belongs to the old gateway; the next refresh loads the new one.
        if old.as_ref().is_some_and(|o| o.endpoint != profile.endpoint) {
            self.gateway_models.remove(&profile.id);
        }
        if !self.persist() {
            return false;
        }
        if let Some(key) = new_key.filter(|k| !k.trim().is_empty()) {
            self.store_key(&profile.id, &key);
        }
        if let Some(old) = old.filter(|o| *o != profile) {
            let _ = old;
            if !self.is_disabled() {
                for binding in self.state.bindings.clone().iter().filter(|b| b.profile_id == profile.id) {
                    self.apply(&profile, &binding.path, Some(binding), None);
                }
                if let Some(global) = self.state.global_binding.clone().filter(|g| g.profile_id == profile.id) {
                    self.apply_global(&profile, Some(&global));
                }
            }
            if self.desktop_status().profile_id.as_deref() == Some(profile.id.as_str()) {
                self.apply_claude_desktop(&profile);
            }
        }
        true
    }

    pub fn store_key(&mut self, id: &str, key: &str) -> bool {
        let Some(profile) = self.profile(id).filter(|p| !p.is_subscription()) else { return false };
        if !self.config.is_endpoint_allowed(&profile.endpoint) {
            self.error = Some(self.not_allowed_message(&profile.endpoint));
            return false;
        }
        match self.keys.write(id, key) {
            Ok(()) => {
                self.keychain_profiles.retain(|p| p != id);
                self.notice = Some(l!(
                    "Key für „%@“ gespeichert. Neue Claude-Sitzungen verwenden ihn sofort, laufende nach Ablauf des Helper-Caches (Standard 5 min), nach einem 401 oder nach Neustart.",
                    profile.name
                ));
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    pub fn delete_profile(&mut self, id: &str) {
        let Some(profile) = self.profile(id) else { return };
        for binding in self.state.bindings.clone().iter().filter(|b| b.profile_id == profile.id) {
            if !self.unbind(&binding.id) {
                return;
            }
        }
        if let Err(e) = self.keys.delete(id).and_then(|_| oidc::logout(&profile, &self.tokens)) {
            self.error = Some(e.to_string());
            return;
        }
        self.sso_expired.remove(id);
        if let Some(global) = self.state.global_binding.clone().filter(|g| g.profile_id == profile.id) {
            if let Some(binder) = &self.binder {
                let _ = binder.revert_global(&global);
            }
            self.state.global_binding = None;
        }
        if self.desktop_status().profile_id.as_deref() == Some(id) {
            let _ = claude_desktop::remove(&self.desktop_library, self.state.claude_desktop_previous_applied.as_deref());
        }
        if self.state.claude_desktop_profile_id.as_deref() == Some(id) {
            self.state.claude_desktop_profile_id = None;
            self.state.claude_desktop_previous_applied = None;
        }
        self.claude_desktop = self.desktop_status();
        self.state.profiles.retain(|p| p.id != id);
        self.budgets.remove(id);
        self.gateway_models.remove(id);
        self.persist();
    }

    /// Signs out locally: removes the tokens (the session at the IdP stays).
    pub fn sso_logout(&mut self, id: &str) {
        let Some(profile) = self.profile(id).filter(|p| p.is_sso()) else { return };
        self.sso_expired.remove(id);
        if let Err(e) = oidc::logout(&profile, &self.tokens) {
            self.error = Some(e.to_string());
        }
    }

    /// Access token of an SSO profile for gateway calls; flags the profile when the session expired.
    pub fn sso_token(&mut self, profile: &Profile) -> Result<String, String> {
        match oidc::access_token(profile, &self.tokens, oidc::unix_now()) {
            Ok(token) => Ok(token),
            Err(e) => {
                if matches!(e, keyzapper_core::errors::Error::SessionExpired(_)) {
                    self.sso_expired.insert(profile.id.clone());
                }
                Err(e.to_string())
            }
        }
    }

    /// The key for copying, or None with an error set.
    pub fn key_for_copy(&mut self, id: &str) -> Option<(String, String)> {
        if !self.config.allow_key_export {
            self.error = Some(l!("Das Kopieren von Keys ist laut Firmenrichtlinie (AllowKeyExport) deaktiviert."));
            return None;
        }
        let profile = self.profile(id).filter(|p| !p.is_sso())?;
        match self.keys.read(id) {
            Ok(key) => Some((key, profile.name)),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// Profiles whose budget the gateway reports, with the stored key; signed-in SSO profiles come without (their token is fetched when needed).
    pub fn budget_profiles(&self) -> Vec<(Profile, Option<String>)> {
        let keys = self.keys.all().unwrap_or_default();
        let tokens = self.tokens.all().unwrap_or_default();
        self.state
            .profiles
            .iter()
            .filter_map(|p| {
                if p.is_sso() {
                    let signed_in = !self.sso_expired.contains(&p.id) && tokens.get(&p.id).is_some_and(|t| !t.refresh_token.is_empty());
                    signed_in.then(|| (p.clone(), None))
                } else {
                    Some((p.clone(), Some(keys.get(&p.id)?.clone())))
                }
            })
            .collect()
    }

    // MARK: Bindings

    pub fn bind(&mut self, folder: &str, profile_id: &str) {
        let Some(profile) = self.profile(profile_id) else { return };
        let root = settings::settings_root(folder);
        let previous = self.state.bindings.iter().find(|b| b.path == root).cloned();
        self.apply(&profile, &root, previous.as_ref(), None);
    }

    pub fn reapply(&mut self, binding_id: &str) {
        let Some(binding) = self.binding(binding_id) else { return };
        let Some(profile) = self.profile(&binding.profile_id) else { return };
        self.apply(&profile, &binding.path, Some(&binding), None);
    }

    /// "Alle reparieren": writes the settings of several projects again, with one notice for all.
    pub fn reapply_many(&mut self, binding_ids: &[String]) {
        let mut repaired = 0;
        for id in binding_ids {
            let Some(binding) = self.binding(id) else { continue };
            let Some(profile) = self.profile(&binding.profile_id) else { continue };
            if self.apply(&profile, &binding.path, Some(&binding), None) {
                repaired += 1;
            }
        }
        if repaired > 0 {
            self.notice = Some(l!("%@ Projekt(e) erneut angewendet. Laufende Claude-Sitzungen dort bitte neu starten.", repaired));
        }
    }

    /// Gives a project the models of one tenant of its profile's gateway (`Some`), or the profile's own models (`None`).
    /// Only the four model keys change; helper and endpoint stay the same. Only SSO profiles have tenants: a key profile
    /// (and a subscription) keeps its own models, as the UI offers no choice there.
    pub fn set_binding_tenant(&mut self, binding_id: &str, tenant: Option<String>) {
        let Some(binding) = self.binding(binding_id) else { return };
        let Some(profile) = self.profile(&binding.profile_id).filter(|p| p.is_sso()) else { return };
        let tenant = tenant.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let selection = match &tenant {
            None => None,
            Some(tenant) => {
                let models = self.gateway_models.get(&profile.id).map(Vec::as_slice).unwrap_or_default();
                match selection_for_tenant(models, tenant, &profile) {
                    Some(selection) => Some(selection),
                    None => {
                        self.error = Some(l!("Das Gateway meldet keine Modelle für den Tenant „%@“.", tenant));
                        return;
                    }
                }
            }
        };
        if self.apply_with(&profile, &binding.path, Some(&binding), None, selection) {
            let folder = folder_name(&binding.path);
            self.notice = Some(match tenant {
                Some(tenant) => l!("„%@“ nutzt jetzt die Modelle von Tenant „%@“. Laufende Claude-Sitzungen in diesem Projekt bitte neu starten.", folder, tenant),
                None => l!("„%@“ nutzt wieder die Modelle des Profils. Laufende Claude-Sitzungen in diesem Projekt bitte neu starten.", folder),
            });
        }
    }

    /// What the tray menu shows; compared as a whole to skip rebuilding an unchanged menu.
    pub fn tray_info(&self) -> TrayInfo {
        let keys = self.keys.all().unwrap_or_default();
        let tokens = self.tokens.all().unwrap_or_default();
        let usable = |p: &Profile| {
            if p.is_subscription() {
                self.subscription.as_ref().is_none_or(|s| s.logged_in)
            } else if p.is_sso() {
                !self.sso_expired.contains(&p.id) && tokens.get(&p.id).is_some_and(|t| !t.refresh_token.is_empty())
            } else {
                keys.get(&p.id).is_some_and(|k| !k.is_empty())
            }
        };
        TrayInfo {
            disabled: self.is_disabled(),
            profiles: self
                .state
                .profiles
                .iter()
                .map(|p| TrayProfile {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    usable: usable(p),
                    budgets: self
                        .budgets
                        .get(&p.id)
                        .map(|list| list.iter().map(|b| TrayBudget { tenant: b.tenant.clone(), spend: b.budget.spend, max: b.budget.max_budget }).collect())
                        .unwrap_or_default(),
                })
                .collect(),
            projects: self
                .state
                .bindings
                .iter()
                .map(|b| TrayProject {
                    id: b.id.clone(),
                    name: folder_name(&b.path),
                    profile_id: b.profile_id.clone(),
                    healthy: self.statuses.get(&b.id).is_some_and(|s| s.health == BindingHealth::Active && !s.conflicts.iter().any(|c| c.blocking)),
                })
                .collect(),
        }
    }

    /// Projects whose Budget-Killer may burn this profile's key: pooled bindings of other profiles on the same endpoint.
    pub fn budget_killers(&self, profile: &Profile) -> Vec<String> {
        if self.is_disabled() || profile.is_sso() || profile.is_subscription() {
            return vec![];
        }
        self.state
            .bindings
            .iter()
            .filter(|b| {
                b.is_pooled()
                    && b.profile_id != profile.id
                    && self.state.profile(&b.profile_id).is_some_and(|p| helper::same_endpoint(&p.endpoint, &profile.endpoint))
            })
            .map(|b| folder_name(&b.path))
            .collect()
    }

    /// Hidden Budget-Killer: switches a project between its own key and the pool of all keys on the same endpoint.
    pub fn toggle_pool(&mut self, binding_id: &str) {
        let Some(binding) = self.binding(binding_id) else { return };
        let Some(profile) = self.profile(&binding.profile_id).filter(|p| !p.is_sso() && !p.is_subscription()) else { return };
        let pooled = !binding.is_pooled();
        if !self.apply(&profile, &binding.path, Some(&binding), Some(pooled)) {
            return;
        }
        let folder = folder_name(&binding.path);
        self.notice = Some(if pooled {
            l!("Budget-Killer für „%@“ aktiv: Ist das Budget von „%@“ verbraucht, verbrennt Claude die Keys der anderen Profile am selben Gateway. Laufende Claude-Sitzungen bitte neu starten.", folder, profile.name)
        } else {
            l!("Budget-Killer für „%@“ aus: Claude nutzt nur noch den Key von „%@“. Laufende Claude-Sitzungen bitte neu starten.", folder, profile.name)
        });
    }

    pub fn unbind(&mut self, binding_id: &str) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        let Some(binder) = self.binder.clone() else { return false };
        let mcp_kept = match mcp::revert(&binding.path, &binding.mcp_managed, binding.mcp_git_exclude_entry.as_deref()) {
            Ok(kept) => kept,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };
        match binder.revert(&binding) {
            Ok(mut kept) => {
                kept.extend(mcp_kept.into_iter().map(|n| format!("mcpServers.{n}")));
                self.state.bindings.retain(|b| b.id != binding.id);
                self.statuses.remove(&binding.id);
                self.persist();
                self.notice = Some(if kept.is_empty() {
                    l!("Zuordnung für „%@“ entfernt. Laufende Claude-Sitzungen dort bitte neu starten.", folder_name(&binding.path))
                } else {
                    l!("Zuordnung entfernt. Außerhalb der App geänderte Einträge wurden beibehalten: %@", kept.join(", "))
                });
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    // MARK: Agent instruction files

    /// Profile, folder and credential for the repository analysis of an assigned project.
    pub fn analysis_access(&mut self, binding_id: &str) -> Result<(Profile, String, String), String> {
        let binding = self.binding(binding_id).ok_or_else(|| l!("Unbekannte Zuordnung."))?;
        let profile = self.profile(&binding.profile_id).ok_or_else(|| l!("Unbekanntes Profil: %@", binding.profile_id))?;
        if profile.is_subscription() {
            return Err(l!("Die Analyse läuft über ein Gateway; mit einem Claude-Abo ist sie nicht verfügbar."));
        }
        if !self.config.is_endpoint_allowed(&profile.endpoint) {
            return Err(self.not_allowed_message(&profile.endpoint));
        }
        let key = if profile.is_sso() { self.sso_token(&profile)? } else { self.keys.read(&profile.id).map_err(|e| e.to_string())? };
        // The analysis is billed like the project's own sessions: with the models of its tenant, if it has chosen one.
        let profile = match &binding.models {
            Some(s) => Profile {
                model_alias: s.model_alias.clone(),
                opus_model: s.opus_model.clone(),
                sonnet_model: s.sonnet_model.clone(),
                haiku_model: s.haiku_model.clone(),
                ..profile
            },
            None => profile,
        };
        Ok((profile, binding.path, key))
    }

    /// Writes the reviewed `AGENTS.md` into the assigned folder.
    pub fn write_agents_file(&mut self, binding_id: &str, content: &str) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        if !Path::new(&binding.path).is_dir() {
            self.error = Some(l!("Ordner nicht gefunden: %@", abbreviate_home(&binding.path)));
            return false;
        }
        let target = Path::new(&binding.path).join(agents::FILE_NAME);
        let shown = abbreviate_home(&target.to_string_lossy());
        match keyzapper_core::paths::write_atomic(&target, format!("{}\n", content.trim_end()).as_bytes(), false) {
            Ok(()) => {
                self.notice = Some(l!("%@ gespeichert. Neue Agenten-Sitzungen im Projekt nutzen die Datei.", shown));
                true
            }
            Err(e) => {
                self.error = Some(l!("%@ konnte nicht gespeichert werden: %@", shown, e));
                false
            }
        }
    }

    /// Writes a project's settings again; its stored tenant choice stays, unless the project moves to another profile
    /// (the models belong to the old profile's gateway).
    fn apply(&mut self, profile: &Profile, folder: &str, previous: Option<&WorkspaceBinding>, pooled: Option<bool>) -> bool {
        let selection = previous.filter(|p| p.profile_id == profile.id).and_then(|p| self.tenant_selection(profile, p.models.as_ref()));
        self.apply_with(profile, folder, previous, pooled, selection)
    }

    /// The tenant models a project keeps when its settings are written again, derived anew from the profile as it is now,
    /// so a change of the profile's models (also by IT) reaches the project. Only an SSO profile has tenants. Without the
    /// gateway's model list (not loaded yet), or when the tenant is no longer listed, the stored models stay: the project is
    /// never moved to another tenant's budget silently.
    fn tenant_selection(&self, profile: &Profile, stored: Option<&ModelSelection>) -> Option<ModelSelection> {
        let stored = stored.filter(|_| profile.is_sso())?;
        let models = self.gateway_models.get(&profile.id);
        models.and_then(|models| selection_for_tenant(models, &stored.tenant, profile)).or_else(|| Some(stored.clone()))
    }

    /// Writes again the projects of `profile_id` whose tenant models no longer follow the profile, once the gateway's model
    /// list is loaded (after a profile change, or at start after IT changed a managed profile).
    pub fn follow_tenants(&mut self, profile_id: &str) {
        let Some(profile) = self.profile(profile_id) else { return };
        if self.is_disabled() {
            return;
        }
        let notice = self.notice.clone();
        for binding in self.state.bindings.clone().iter().filter(|b| b.profile_id == profile_id && b.models.is_some()) {
            if self.tenant_selection(&profile, binding.models.as_ref()) != binding.models && Path::new(&binding.path).is_dir() {
                self.apply(&profile, &binding.path, Some(binding), None);
            }
        }
        self.notice = notice;
    }

    fn apply_with(&mut self, profile: &Profile, folder: &str, previous: Option<&WorkspaceBinding>, pooled: Option<bool>, selection: Option<ModelSelection>) -> bool {
        let Some(binder) = self.binder.clone() else { return false };
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return false;
        }
        let pooled = pooled.unwrap_or_else(|| previous.is_some_and(|p| p.is_pooled()));
        match binder.apply(profile, folder, previous, pooled, selection.as_ref()) {
            Ok(result) => {
                let binding_id = result.binding.id.clone();
                let has_mcp = !result.binding.mcp_server_ids.is_empty() || !result.binding.mcp_managed.is_empty();
                match self.state.bindings.iter_mut().find(|b| b.id == result.binding.id) {
                    Some(existing) => *existing = result.binding,
                    None => self.state.bindings.push(result.binding),
                }
                self.persist();
                if has_mcp {
                    self.apply_mcp(&binding_id, true);
                }
                self.notice = Some(if result.changed {
                    l!("„%@“ verwendet jetzt Profil „%@“. Laufende Claude-Sitzungen in diesem Projekt bitte neu starten.", folder_name(folder), profile.name)
                } else {
                    l!("„%@“ ist bereits eingerichtet – keine Änderungen.", folder_name(folder))
                });
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    fn persist(&mut self) -> bool {
        if self.metadata_unreadable {
            self.error = Some(l!("Metadaten konnten nicht gelesen werden; Änderungen werden nicht gespeichert."));
            return false;
        }
        if let Err(e) = self.metadata.save(&self.state) {
            self.error = Some(e.to_string());
            return false;
        }
        self.write_backup();
        true
    }

    // MARK: MCP servers

    fn mcp_server(&self, id: &str) -> Option<McpServer> {
        self.state.mcp_server(id).cloned()
    }

    /// Server of the central list; IT changes it there, not in the app.
    fn is_managed_mcp(&self, id: &str) -> bool {
        self.mcp_catalog.as_ref().is_some_and(|c| c.servers.iter().any(|s| s.id == id))
    }

    pub fn save_mcp_server(&mut self, input: McpServerInput) -> bool {
        let name = input.name.trim().to_string();
        let existing = input.id.as_deref().and_then(parse_id).and_then(|id| self.mcp_server(&id));
        if existing.as_ref().is_some_and(|e| self.is_managed_mcp(&e.id)) {
            self.error = Some(l!("Dieser MCP-Server wird von der IT verwaltet und kann nicht geändert werden."));
            return false;
        }
        if !McpServer::is_valid_name(&name) {
            self.error = Some(l!("Der Servername darf nur Buchstaben, Ziffern, „-“, „_“ und „.“ enthalten."));
            return false;
        }
        if self.state.mcp_servers.iter().any(|s| s.name == name && existing.as_ref().is_none_or(|e| e.id != s.id)) {
            self.error = Some(l!("Es gibt bereits einen MCP-Server namens „%@“.", name));
            return false;
        }
        let (env, invalid_env) = McpServer::parse_pairs(&input.env_text, '=');
        let (headers, invalid_headers) = McpServer::parse_pairs(&input.headers_text, ':');
        if !invalid_env.is_empty() || !invalid_headers.is_empty() {
            return false;
        }
        let mut server = existing.unwrap_or_else(|| McpServer::new(new_id(), name.clone()));
        server.name = name;
        server.transport = input.transport;
        server.command = String::new();
        server.args = vec![];
        server.env = BTreeMap::new();
        server.url = String::new();
        server.headers = BTreeMap::new();
        server.oauth_client_id = String::new();
        server.oauth_callback_port = None;
        server.oauth_scopes = String::new();
        match input.transport {
            McpTransport::Stdio => {
                let command = input.command.trim().to_string();
                if command.is_empty() {
                    return false;
                }
                server.command = command;
                server.args = McpServer::parse_args(&input.args_text);
                server.env = env;
            }
            McpTransport::Http | McpTransport::Sse => {
                let Some(url) = valid_endpoint(&input.url) else {
                    self.error = Some(l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."));
                    return false;
                };
                server.url = url;
                server.headers = headers;
                server.oauth_client_id = input.oauth_client_id.trim().to_string();
                server.oauth_scopes = input.oauth_scopes.split_whitespace().collect::<Vec<_>>().join(" ");
                let port = input.oauth_callback_port.trim();
                if !port.is_empty() {
                    match port.parse::<u16>() {
                        Ok(port) if port > 0 => server.oauth_callback_port = Some(port),
                        _ => {
                            self.error = Some(l!("Der Callback-Port muss eine Zahl zwischen 1 und 65535 sein."));
                            return false;
                        }
                    }
                }
            }
        }
        let old = self.mcp_server(&server.id);
        match self.state.mcp_servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server.clone(),
            None => self.state.mcp_servers.push(server.clone()),
        }
        if !self.persist() {
            return false;
        }
        self.notice = Some(l!("MCP-Server „%@“ gespeichert.", server.name));
        if old.is_some_and(|o| o != server) {
            for binding in self.state.bindings.clone().iter().filter(|b| b.mcp_server_ids.contains(&server.id)) {
                self.apply_mcp(&binding.id, false);
            }
        }
        true
    }

    pub fn delete_mcp_server(&mut self, id: &str) {
        let Some(server) = self.mcp_server(id) else { return };
        if self.is_managed_mcp(id) {
            self.error = Some(l!("Dieser MCP-Server wird von der IT verwaltet und kann nicht geändert werden."));
            return;
        }
        for binding in self.state.bindings.clone().iter().filter(|b| b.mcp_server_ids.contains(&server.id)) {
            let ids = binding.mcp_server_ids.iter().filter(|i| **i != server.id).cloned().collect();
            if !self.set_project_mcp_servers(&binding.id, ids) {
                return;
            }
        }
        self.state.mcp_servers.retain(|s| s.id != server.id);
        self.persist();
        self.notice = Some(l!("MCP-Server „%@“ gelöscht.", server.name));
    }

    /// Chooses the servers a project uses and writes them; on failure the previous choice is kept.
    pub fn set_project_mcp_servers(&mut self, binding_id: &str, ids: Vec<String>) -> bool {
        let ids: Vec<String> = ids.into_iter().filter(|id| self.state.mcp_server(id).is_some()).collect();
        let Some(binding) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) else { return false };
        let previous = std::mem::replace(&mut binding.mcp_server_ids, ids);
        if self.apply_mcp(binding_id, false) {
            return true;
        }
        if let Some(binding) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) {
            binding.mcp_server_ids = previous;
        }
        false
    }

    /// Writes the project's MCP servers into its `.mcp.json`; `quiet` keeps the current notice.
    fn apply_mcp(&mut self, binding_id: &str, quiet: bool) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        let servers: Vec<McpServer> = binding.mcp_server_ids.iter().filter_map(|id| self.mcp_server(id)).collect();
        match mcp::apply(&binding.path, &servers, &binding.mcp_managed, binding.mcp_git_exclude_entry.as_deref()) {
            Ok(result) => {
                let names: Vec<String> = result.managed.keys().cloned().collect();
                if let Some(b) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) {
                    b.mcp_managed = result.managed;
                    b.mcp_git_exclude_entry = result.git_exclude_entry;
                }
                self.persist();
                if !quiet {
                    let folder = folder_name(&binding.path);
                    self.notice = Some(if names.is_empty() {
                        l!("MCP-Server für „%@“ entfernt. Laufende Claude-Sitzungen dort bitte neu starten.", folder)
                    } else if result.changed {
                        l!("„%@“ nutzt jetzt die MCP-Server %@. Laufende Claude-Sitzungen dort bitte neu starten.", folder, names.join(", "))
                    } else {
                        l!("MCP-Server für „%@“ sind bereits eingerichtet – keine Änderungen.", folder)
                    });
                }
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    /// Takes over MCP servers from a backup (new ones added, known ones updated; those of the central list stay as IT set them).
    fn merge_mcp_servers(&mut self, servers: &[McpServer]) {
        for server in servers {
            if self.is_managed_mcp(&server.id) {
                continue;
            }
            match self.state.mcp_servers.iter_mut().find(|s| s.id == server.id) {
                Some(existing) => *existing = server.clone(),
                None => self.state.mcp_servers.push(server.clone()),
            }
        }
    }

    // MARK: Central MCP server list

    /// Address of the central list and the token of an SSO profile signed in at its host (gateway or identity
    /// provider), so the server can answer per user. Only hosts that get this token anyway receive it.
    pub fn mcp_catalog_request(&mut self) -> Option<(String, Option<String>)> {
        let url = self.config.managed_mcp_servers_url.clone()?;
        let host = host_of(&url);
        let now = oidc::unix_now();
        let token = self
            .state
            .profiles
            .iter()
            .filter(|p| p.is_sso() && host.is_some() && [host_of(&p.endpoint), p.oidc_issuer.as_deref().and_then(host_of)].contains(&host))
            .find_map(|p| oidc::access_token(p, &self.tokens, now).ok());
        self.mcp_catalog_loading = true;
        Some((url, token))
    }

    pub fn take_mcp_catalog(&mut self, result: Result<Catalog, FetchError>) {
        self.mcp_catalog_loading = false;
        match result {
            Ok(catalog) => {
                if let Err(e) = mcp_catalog::save_cache(&catalog) {
                    eprintln!("KeyZapper: MCP list not cached: {e}");
                }
                let previous = self.mcp_catalog.replace(catalog);
                self.mcp_catalog_problem = None;
                self.sync_managed_mcp_servers(previous.as_ref());
            }
            Err(e) => self.mcp_catalog_problem = Some(e.to_string()),
        }
    }

    /// Takes over the central list: new servers are added, changed ones rewritten in the projects using them, and
    /// those IT took off the list (compared with `previous`) leave the projects. A server of your own with the same
    /// name keeps its place; the list's entry is then left out.
    fn sync_managed_mcp_servers(&mut self, previous: Option<&Catalog>) {
        let Some(catalog) = self.mcp_catalog.clone() else { return };
        let notice = self.notice.clone();
        let (mut added, mut changed) = (false, Vec::new());
        for server in &catalog.servers {
            if self.state.mcp_servers.iter().any(|s| s.name == server.name && s.id != server.id) {
                continue;
            }
            match self.state.mcp_servers.iter_mut().find(|s| s.id == server.id) {
                Some(existing) if existing == server => {}
                Some(existing) => {
                    *existing = server.clone();
                    changed.push(server.id.clone());
                }
                None => {
                    self.state.mcp_servers.push(server.clone());
                    added = true;
                }
            }
        }
        if added || !changed.is_empty() {
            self.persist();
        }
        for binding in self.state.bindings.clone().iter().filter(|b| b.mcp_server_ids.iter().any(|id| changed.contains(id))) {
            self.apply_mcp(&binding.id, true);
        }
        for gone in previous.into_iter().flat_map(|p| p.servers.iter()).filter(|s| !catalog.servers.iter().any(|c| c.id == s.id)) {
            self.delete_mcp_server(&gone.id);
        }
        self.notice = notice;
    }

    /// Names on the central list that were left out because a server of your own has that name.
    fn shadowed_mcp_names(&self) -> Vec<String> {
        let Some(catalog) = &self.mcp_catalog else { return vec![] };
        catalog.servers.iter().filter(|c| self.state.mcp_server(&c.id).is_none()).map(|c| c.name.clone()).collect()
    }

    // MARK: Claude tools

    pub fn tool_installed_message(tool: Tool) -> String {
        match tool {
            Tool::Cli => l!("Claude Code ist installiert. Neue Terminal-Fenster finden den Befehl claude."),
            Tool::VsCode => l!("Claude Code für VS Code ist installiert. Bitte das VS Code-Fenster neu laden."),
            Tool::Intellij => l!("Das Claude Code-Plugin ist installiert. Bitte IntelliJ IDEA neu starten."),
            Tool::Desktop => l!("Claude Desktop ist installiert."),
        }
    }

    /// Lets Claude Desktop use a profile's gateway (key or SSO via the helper, or its own sign-in with the profile's
    /// `DesktopSso` block); None restores its normal sign-in.
    pub fn set_claude_desktop_profile(&mut self, profile_id: Option<String>) {
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return;
        }
        let profile = match profile_id {
            Some(id) => match self.profile(&id) {
                Some(profile) => Some(profile),
                None => {
                    self.error = Some(l!("Unbekanntes Profil: %@", id));
                    return;
                }
            },
            None => None,
        };
        match profile {
            Some(profile) if profile.is_subscription() => {
                self.error = Some(l!("Mit einem Claude-Abo meldet sich Claude Desktop selbst an. Bitte „Normale Anmeldung“ wählen."));
            }
            Some(profile) if !self.is_allowed(&profile) => self.error = Some(self.not_allowed_message(&profile.endpoint)),
            Some(profile) => {
                if self.apply_claude_desktop(&profile) {
                    self.notice = Some(l!("Claude Desktop nutzt jetzt „%@“. Bitte Claude Desktop beenden und neu öffnen.", profile.name));
                }
            }
            None => match claude_desktop::remove(&self.desktop_library, self.state.claude_desktop_previous_applied.as_deref()) {
                Ok(()) => {
                    self.state.claude_desktop_profile_id = None;
                    self.state.claude_desktop_previous_applied = None;
                    self.persist();
                    self.notice = Some(l!("KeyZapper-Konfiguration aus Claude Desktop entfernt. Nach einem Neustart meldet sich Claude Desktop wieder normal an."));
                }
                Err(e) => self.error = Some(l!("Claude Desktop konnte nicht eingerichtet werden: %@", e)),
            },
        }
        self.claude_desktop = self.desktop_status();
    }

    /// Records how the background installation of Claude Desktop ended (None = installed).
    pub fn record_desktop_install(&mut self, error: Option<String>) {
        let error = error.map(|e| l!("Claude Desktop konnte nicht installiert werden: %@", e));
        if self.state.claude_desktop_install_error != error {
            self.state.claude_desktop_install_error = error;
            self.persist();
        }
    }

    /// What Claude Desktop is set to; the record names the profile of the sign-in variant, whose entry does not.
    fn desktop_status(&self) -> claude_desktop::Status {
        claude_desktop::status(&self.desktop_library, self.state.claude_desktop_profile_id.as_deref())
    }

    /// Claude Desktop with the one IT profile whose `ClaudeDesktop` block has `AutoApply`: written and applied without a click,
    /// and written again at start and refresh when the entry was deleted or edited or another one was applied. Never while
    /// KeyZapper is deactivated or IT configures Claude Desktop itself; without `AutoApply` nothing is touched unprompted.
    pub fn auto_apply_claude_desktop(&mut self) {
        self.desktop_problem = None;
        if self.is_disabled() || self.desktop_status().managed {
            return;
        }
        let auto: Vec<Profile> = self
            .state
            .profiles
            .iter()
            .filter(|p| self.config.is_managed(&p.id) && p.claude_desktop.as_ref().is_some_and(|d| d.auto_apply))
            .cloned()
            .collect();
        let profile = match auto.as_slice() {
            [] => return,
            [profile] => profile.clone(),
            _ => {
                self.desktop_problem = Some(l!("Mehrere Profile der IT sollen Claude Desktop automatisch einrichten; KeyZapper richtet deshalb keins ein."));
                return;
            }
        };
        let Some(helper) = self.binder.as_ref().map(|b| b.helper_path.clone()) else { return };
        if claude_desktop::is_current(&self.desktop_library, &claude_desktop::configuration(&profile, &helper)) {
            return;
        }
        // Reported in the Claude Desktop settings, not as a message at start.
        let error = self.error.take();
        if !self.apply_claude_desktop(&profile) {
            self.desktop_problem = self.error.take();
        }
        self.error = error;
        self.claude_desktop = self.desktop_status();
    }

    /// Writes KeyZapper's entry for `profile` and records it. Not while IT configures Claude Desktop: its settings win,
    /// and an entry of KeyZapper would only mislead.
    fn apply_claude_desktop(&mut self, profile: &Profile) -> bool {
        let Some(helper) = self.binder.as_ref().map(|b| b.helper_path.clone()) else { return false };
        if self.desktop_status().managed {
            self.error = Some(l!("Claude Desktop wird auf diesem Rechner von der IT konfiguriert; KeyZapper richtet es deshalb nicht ein."));
            return false;
        }
        if let Some(sso) = &profile.claude_desktop {
            let mut problems = sso.problems();
            if problems.is_empty() && !self.config.is_endpoint_allowed(sso.gateway(profile)) {
                problems.push(self.not_allowed_message(sso.gateway(profile)));
            }
            if !problems.is_empty() {
                self.error = Some(l!("Claude Desktop konnte nicht eingerichtet werden: %@", problems.join(" ")));
                return false;
            }
        }
        // What Claude Desktop applied before KeyZapper's entry comes back when KeyZapper removes it.
        let previous = claude_desktop::applied_id(&self.desktop_library);
        let previous = if previous.as_deref() == Some(claude_desktop::ENTRY_ID) { self.state.claude_desktop_previous_applied.clone() } else { previous };
        match claude_desktop::apply(&self.desktop_library, profile, &helper) {
            Ok(()) => {
                if self.state.claude_desktop_profile_id.as_deref() != Some(profile.id.as_str()) || self.state.claude_desktop_previous_applied != previous {
                    self.state.claude_desktop_profile_id = Some(profile.id.clone());
                    self.state.claude_desktop_previous_applied = previous;
                    self.persist();
                }
                true
            }
            Err(e) => {
                self.error = Some(l!("Claude Desktop konnte nicht eingerichtet werden: %@", e));
                false
            }
        }
    }

    // MARK: OneDrive backup (profiles and bindings only, never keys)

    fn write_backup(&mut self) {
        let Some(dir) = self.backup_dir.clone() else { return };
        if self.available_backup.is_some() {
            return;
        }
        match BackupStore::new(dir).write(&self.state) {
            Ok(()) => self.backup_problem = None,
            Err(e) => self.backup_problem = Some(l!("OneDrive-Backup fehlgeschlagen: %@", e)),
        }
    }

    pub fn restore_from_backup(&mut self) {
        let Some(backup) = self.available_backup.take() else { return };
        for profile in &backup.profiles {
            if self.state.profile(&profile.id).is_none() {
                self.state.profiles.push(profile.clone());
            }
        }
        self.merge_mcp_servers(&backup.mcp_servers);
        if !self.persist() {
            return;
        }
        let mut restored = 0;
        for binding in &backup.bindings {
            let Some(profile) = self.profile(&binding.profile_id) else { continue };
            if Path::new(&binding.path).exists() && self.apply(&profile, &binding.path, Some(binding), None) {
                restored += 1;
            }
        }
        let skipped = backup.bindings.len() - restored;
        let mut notice = l!("Backup wiederhergestellt: %@ Profil(e), %@ Projekt(e)", backup.profiles.len(), restored);
        if skipped > 0 {
            notice += &l!(", %@ übersprungen (Ordner fehlt oder Konflikt)", skipped);
        }
        notice += &l!(". Keys werden nicht gesichert – bitte je Profil neu eintragen.");
        self.notice = Some(notice);
    }

    pub fn discard_backup(&mut self) {
        self.available_backup = None;
        self.write_backup();
    }

    // MARK: Encrypted backup file (keys and settings)

    /// Profiles, assignments and (unless IT forbids it) the keys.
    pub fn backup_payload(&self) -> BackupPayload {
        let keys = if self.config.allow_key_export {
            let all = self.keys.all().unwrap_or_default();
            self.state.profiles.iter().filter(|p| !p.is_sso()).filter_map(|p| Some((p.id.clone(), all.get(&p.id)?.clone()))).collect()
        } else {
            BTreeMap::new()
        };
        BackupPayload {
            created_at: keyzapper_core::time::iso8601_utc(),
            app_version: self.app_version.clone(),
            state: self.state.clone(),
            keys,
        }
    }

    /// Restores profiles and keys from an encrypted backup and re-assigns projects whose folders exist here.
    pub fn import_payload(&mut self, payload: BackupPayload) -> bool {
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return false;
        }
        let mut skipped = 0;
        let mut changed_profiles = Vec::new();
        for profile in &payload.state.profiles {
            if !self.is_allowed(profile) {
                skipped += 1;
                continue;
            }
            let managed = self.config.is_managed(&profile.id);
            match self.state.profiles.iter_mut().find(|p| p.id == profile.id) {
                Some(existing) => {
                    if !managed && existing != profile {
                        *existing = profile.clone();
                        // The cached model list belongs to the old gateway; the next refresh loads the new one.
                        self.gateway_models.remove(&profile.id);
                        changed_profiles.push(profile.id.clone());
                    }
                }
                None => self.state.profiles.push(profile.clone()),
            }
        }
        self.merge_mcp_servers(&payload.state.mcp_servers);
        if !self.persist() {
            return false;
        }
        let mut key_count = 0;
        for (id, key) in &payload.keys {
            let Some(id) = parse_id(id) else { continue };
            if self.state.profile(&id).is_some() && self.keys.write(&id, key).is_ok() {
                key_count += 1;
                self.keychain_profiles.retain(|p| *p != id);
            }
        }
        let mut project_count = 0;
        for binding in &payload.state.bindings {
            let profile = self.profile(&binding.profile_id);
            let Some(profile) = profile.filter(|_| Path::new(&binding.path).exists()) else {
                skipped += 1;
                continue;
            };
            let previous = self.state.bindings.iter().find(|b| b.path == binding.path).cloned().unwrap_or_else(|| binding.clone());
            let selection = self.tenant_selection(&profile, binding.models.as_ref());
            if self.apply_with(&profile, &binding.path, Some(&previous), None, selection) {
                project_count += 1;
            } else {
                skipped += 1;
            }
        }
        // Existing assignments of profiles the backup changed must pick up the new endpoint and models too.
        let imported: Vec<&String> = payload.state.bindings.iter().map(|b| &b.path).collect();
        for binding in self.state.bindings.clone() {
            if changed_profiles.contains(&binding.profile_id) && !imported.contains(&&binding.path) {
                if let Some(profile) = self.profile(&binding.profile_id) {
                    self.apply(&profile, &binding.path, Some(&binding), None);
                }
            }
        }
        if let Some(global) = self.state.global_binding.clone().filter(|g| changed_profiles.contains(&g.profile_id)) {
            if let Some(profile) = self.profile(&global.profile_id) {
                self.apply_global(&profile, Some(&global));
            }
        }
        let mut notice = l!(
            "Backup importiert: %@ Profil(e), %@ Key(s), %@ Projekt(e).",
            payload.state.profiles.len(),
            key_count,
            project_count
        );
        if skipped > 0 {
            notice += &l!(" %@ Eintrag/Einträge übersprungen (Ordner fehlt, Konflikt oder nicht freigegebener Endpunkt).", skipped);
        }
        self.notice = Some(notice);
        true
    }

    // MARK: ~/.claude/settings.json (default profile, repair) and deactivation

    /// Sets (or with None removes) the default profile in `~/.claude/settings.json` for folders without assignment.
    pub fn set_global_profile(&mut self, profile_id: Option<String>) {
        let Some(binder) = self.binder.clone() else { return };
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return;
        }
        if let Some(profile) = profile_id.and_then(|id| self.profile(&id)) {
            let backup = if self.state.global_binding.is_none() { binder.backup_user_settings().ok().flatten() } else { None };
            let previous = self.state.global_binding.clone();
            if self.apply_global(&profile, previous.as_ref()) {
                let mut notice =
                    l!("Standardprofil „%@“ in ~/.claude/settings.json eingetragen. Es gilt für alle Ordner ohne eigene Zuordnung.", profile.name);
                if let Some(backup) = backup {
                    notice += &l!(" Sicherung: %@", file_name(&backup));
                }
                self.notice = Some(notice);
            }
        } else if let Some(global) = self.state.global_binding.clone() {
            match binder.revert_global(&global) {
                Ok(_) => {
                    self.state.global_binding = None;
                    self.persist();
                    self.notice = Some(l!("Standardprofil aus ~/.claude/settings.json entfernt."));
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    fn apply_global(&mut self, profile: &Profile, previous: Option<&WorkspaceBinding>) -> bool {
        let Some(binder) = self.binder.clone() else { return false };
        match binder.apply_global(profile, previous) {
            Ok(result) => {
                self.state.global_binding = Some(result.binding);
                self.persist();
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    /// Backs up and cleans `~/.claude/settings.json` (valid JSON, no plaintext keys) or creates it.
    pub fn repair_user_settings(&mut self) {
        let Some(binder) = self.binder.clone() else { return };
        match binder.repair_user_settings() {
            Ok(backup) => {
                if !self.is_disabled() {
                    if let Some(global) = self.state.global_binding.clone() {
                        if let Some(profile) = self.profile(&global.profile_id) {
                            self.apply_global(&profile, Some(&global));
                        }
                    }
                }
                let mut notice = l!("~/.claude/settings.json ist jetzt gültig und frei von Klartext-Keys.");
                if let Some(backup) = backup {
                    notice += &l!(" Sicherung: %@", file_name(&backup));
                }
                self.notice = Some(notice);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Deactivation removes KeyZapper's values from all projects and the user settings but keeps the assignments,
    /// so activating writes them again. Claude Code meanwhile uses its normal login everywhere.
    pub fn set_disabled(&mut self, disable: bool) {
        let Some(binder) = self.binder.clone() else { return };
        if disable == self.is_disabled() {
            return;
        }
        if disable {
            let failed: Vec<String> =
                self.state.bindings.iter().filter(|b| binder.revert(b).is_err()).map(|b| folder_name(&b.path)).collect();
            if let Some(global) = &self.state.global_binding {
                let _ = binder.revert_global(global);
            }
            // Claude Desktop goes back to its normal sign-in too; the record stays for the activation.
            if self.state.claude_desktop_profile_id.is_some() {
                if let Err(e) = claude_desktop::remove(&self.desktop_library, self.state.claude_desktop_previous_applied.as_deref()) {
                    self.error = Some(l!("Claude Desktop konnte nicht eingerichtet werden: %@", e));
                }
            }
            self.state.disabled = Some(true);
            self.persist();
            self.claude_desktop = self.desktop_status();
            self.notice = Some(l!("KeyZapper ist deaktiviert. Claude Code nutzt jetzt überall die normale Anmeldung; laufende Sitzungen bitte neu starten."));
            if !failed.is_empty() {
                self.error = Some(l!("Diese Projekte konnten nicht zurückgesetzt werden: %@", failed.join(", ")));
            }
        } else {
            self.state.disabled = None;
            self.persist();
            for binding in self.state.bindings.clone() {
                if let Some(profile) = self.profile(&binding.profile_id) {
                    self.apply(&profile, &binding.path, Some(&binding), None);
                }
            }
            if let Some(global) = self.state.global_binding.clone() {
                if let Some(profile) = self.profile(&global.profile_id) {
                    self.apply_global(&profile, Some(&global));
                }
            }
            if let Some(profile) = self.state.claude_desktop_profile_id.clone().and_then(|id| self.profile(&id)) {
                self.apply_claude_desktop(&profile);
                self.claude_desktop = self.desktop_status();
            }
            self.auto_apply_claude_desktop();
            self.notice = Some(l!("KeyZapper ist wieder aktiv. Laufende Claude-Sitzungen bitte neu starten."));
        }
    }

    // MARK: Keys from the macOS keychain (KeyZapper ≤ 1.x)

    /// Profile IDs without a key in the file, for the (prompt-free) keychain presence check.
    pub fn profiles_without_key(&self) -> Vec<String> {
        let keys = self.keys.all().unwrap_or_default();
        self.state.profiles.iter().filter(|p| !keys.contains_key(&p.id)).map(|p| p.id.clone()).collect()
    }

    /// Moves the keys over; macOS may ask once per key.
    pub fn migrate_keychain(&mut self) {
        let mut moved = 0;
        for id in self.keychain_profiles.clone() {
            if let Some(key) = legacy_keychain::take_key(&id) {
                if self.keys.write(&id, &key).is_ok() {
                    moved += 1;
                    self.keychain_profiles.retain(|p| *p != id);
                }
            }
        }
        if self.keychain_profiles.is_empty() {
            self.notice = Some(l!("%@ Key(s) aus dem Schlüsselbund übernommen. KeyZapper nutzt den Schlüsselbund nicht mehr.", moved));
        } else {
            self.error = Some(l!("%@ Key(s) konnten nicht aus dem Schlüsselbund gelesen werden. Bitte über „Key hinterlegen“ neu eintragen.", self.keychain_profiles.len()));
        }
    }

    // MARK: Claude in Office

    /// The profile Claude in Office is to be set up from: KeyZapper active and a profile with Office switched on, those
    /// from IT first.
    fn office_candidate(&self) -> Option<&Profile> {
        if self.is_disabled() {
            return None;
        }
        let managed: Vec<String> = self.config.profiles.iter().map(|p| p.id.clone()).collect();
        office_profile(&self.state.profiles, &managed)
    }

    /// Why `office` cannot be used yet: unusable settings, or a gateway the company policy does not allow. None = usable.
    fn office_blocker(&self, office: &OfficeClient) -> Option<String> {
        let problems = office.problems();
        if !problems.is_empty() {
            return Some(problems.join(" "));
        }
        (!self.config.is_endpoint_allowed(&office.gateway_url)).then(|| self.not_allowed_message(&office.gateway_url))
    }

    /// True while a run would write a manifest, so the caller fetches Anthropic's current template first (outside the lock).
    pub fn office_wanted(&self) -> bool {
        self.office_candidate().and_then(|p| p.office.as_ref()).is_some_and(|o| self.office_blocker(o).is_none())
    }

    /// Something is wrong with Claude in Office, or no Office app could take the manifest yet (none started once); a
    /// refresh then tries again.
    pub fn office_failed(&self) -> bool {
        self.office_problem.is_some() || self.state.office_error.is_some() || self.state.office_applied.as_ref().is_some_and(|a| a.apps.is_empty())
    }

    /// Decides what a sync run does: compares what the Office profile asks for with the record of what was registered
    /// (never with the app folders). `fetched` is Anthropic's template as just loaded, `None` if it was not tried. `retry`
    /// (a settings change or refresh, not the start) writes again after a failed registration. Returns the job to run
    /// outside the lock, or None when there is nothing to touch; then the status already tells why.
    pub fn plan_office(&mut self, fetched: Option<Result<String, String>>, retry: bool) -> Option<SyncJob> {
        self.office_problem = None;
        let mut manifest = String::new();
        let mut apps = vec![];
        let candidate = self.office_candidate().and_then(|p| Some((p.id.clone(), p.office.clone()?)));
        let desired = match candidate {
            None => None,
            // A gateway the company policy does not allow (any more): nothing is written, and what is registered goes.
            Some((_, office)) if office.problems().is_empty() && !self.config.is_endpoint_allowed(&office.gateway_url) => {
                self.office_problem = Some(self.not_allowed_message(&office.gateway_url));
                None
            }
            Some((profile_id, office)) => {
                let built = match self.office_blocker(&office) {
                    Some(message) => Err(message),
                    None => claude_office::manifest_from(&self.office_template, fetched, &office),
                };
                match built {
                    Ok(xml) => {
                        apps = self.office_sideload.apps();
                        let desired = DesiredOffice::new(&profile_id, &office, &xml, &apps);
                        manifest = xml;
                        Some(desired)
                    }
                    // Nothing is written, and what is registered stays.
                    Err(message) => {
                        self.office_problem = Some(message);
                        return None;
                    }
                }
            }
        };
        let applied = self.state.office_applied.clone();
        let action = if retry && self.state.office_error.is_some() {
            plan_retry(desired.as_ref(), applied.as_ref())
        } else {
            plan_sync(desired.as_ref(), applied.as_ref())
        };
        if action == SyncAction::Nothing {
            return None;
        }
        Some(SyncJob { action, desired, manifest, apps, previous: applied })
    }

    /// Saves `outcome` as the record of what is registered. A run saves its intended record this way before it writes (see
    /// `SyncJob::run`); false if it could not be saved.
    pub fn record_office(&mut self, outcome: &Outcome) -> bool {
        self.state.office_applied = outcome.applied.clone();
        self.state.office_error = outcome.error.clone();
        self.persist()
    }

    /// Records what `job.run` left behind.
    pub fn finish_office(&mut self, job: &SyncJob, outcome: Outcome) {
        self.record_office(&outcome);
        // A failed removal has no profile card to show it on (deactivated, or Office switched off).
        if let (SyncAction::Remove { .. }, Some(error)) = (&job.action, outcome.error) {
            self.error = Some(error);
        }
    }

    fn office_view(&self) -> OfficeView {
        let error = self.office_problem.as_deref().or(self.state.office_error.as_deref());
        OfficeView { status: claude_office::status(self.state.office_applied.as_ref(), error), profile_id: self.office_candidate().map(|p| p.id.clone()) }
    }

    // MARK: View for the web UI

    pub fn view(&self) -> View {
        let keys = self.keys.all().unwrap_or_default();
        let tokens = self.tokens.all().unwrap_or_default();
        let user_settings_path = self.binder.as_ref().map(|b| b.user_settings_path.clone()).unwrap_or_else(settings::default_user_settings_path);
        View {
            platform: if cfg!(windows) { "windows" } else { "macos" },
            language: keyzapper_core::i18n::language(),
            app_version: self.app_version.clone(),
            disabled: self.is_disabled(),
            profiles: self
                .state
                .profiles
                .iter()
                .map(|p| {
                    let key = keys.get(&p.id).filter(|k| !k.is_empty());
                    let session = tokens.get(&p.id).filter(|t| !t.refresh_token.is_empty());
                    ProfileView {
                        sso: p.is_sso(),
                        subscription: p.is_subscription(),
                        oidc_issuer: p.oidc_issuer.clone(),
                        oidc_client_id: p.oidc_client_id.clone(),
                        oidc_scope: p.oidc_scope.clone(),
                        oidc_token_type: p.oidc_token_type,
                        sso_state: p.is_sso().then_some(if self.sso_expired.contains(&p.id) {
                            "expired"
                        } else if session.is_some() {
                            "loggedIn"
                        } else {
                            "loggedOut"
                        }),
                        sso_expires_at: session.and_then(|t| t.expires_at),
                        sso_logging_in: self.sso_logging_in.contains(&p.id),
                        id: p.id.clone(),
                        name: p.name.clone(),
                        endpoint: p.endpoint.clone(),
                        model_alias: p.model_alias.clone(),
                        opus_model: p.opus_model.clone(),
                        sonnet_model: p.sonnet_model.clone(),
                        haiku_model: p.haiku_model.clone(),
                        environment_text: Profile::format_environment(p.environment.as_ref()),
                        managed: self.config.is_managed(&p.id),
                        office_managed: self.config.profiles.iter().any(|m| m.id == p.id && m.office.is_some()),
                        claude_desktop_managed: self.config.profiles.iter().any(|m| m.id == p.id && m.claude_desktop.is_some()),
                        has_key: key.is_some(),
                        key_hint: key.map(|k| masked_hint(k)),
                        budgets: self
                            .budgets
                            .get(&p.id)
                            .filter(|_| if p.is_sso() { session.is_some() && !self.sso_expired.contains(&p.id) } else { key.is_some() })
                            .map(|list| {
                                list.iter()
                                    .map(|b| BudgetView {
                                        tenant: b.tenant.clone(),
                                        spend: b.budget.spend,
                                        max_budget: b.budget.max_budget,
                                        remaining: b.budget.max_budget.map(|_| b.budget.remaining()),
                                        reset_at: b.budget.reset_at.clone(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        killers: self.budget_killers(p),
                        // Only an SSO profile has tenants (its model list comes with the user's token). A key profile's
                        // `<x>/<model>` IDs are providers of one gateway, not accounts to choose from.
                        tenants: self.gateway_models.get(&p.id).filter(|_| p.is_sso()).map(|m| tenants_of(m)).unwrap_or_default(),
                        office: p.office.clone(),
                        claude_desktop: p.claude_desktop.clone(),
                    }
                })
                .collect(),
            bindings: self
                .state
                .bindings
                .iter()
                .map(|b| BindingView {
                    id: b.id.clone(),
                    path: b.path.clone(),
                    display_path: abbreviate_home(&b.path),
                    name: folder_name(&b.path),
                    profile_id: b.profile_id.clone(),
                    pooled: b.is_pooled(),
                    tenant: b.models.as_ref().map(|m| m.tenant.clone()),
                    mcp_server_ids: b.mcp_server_ids.clone(),
                    mcp_names: b.mcp_server_ids.iter().filter_map(|id| self.state.mcp_server(id)).map(|s| s.name.clone()).collect(),
                    mcp_active: self.mcp_active.get(&b.id).copied().unwrap_or(true),
                    status: self.statuses.get(&b.id).map(|s| StatusView {
                        health: HealthView::from(&s.health),
                        conflicts: s.conflicts.iter().map(|c| ConflictView { blocking: c.blocking, message: c.message.clone() }).collect(),
                    }),
                })
                .collect(),
            mcp_servers: self
                .state
                .mcp_servers
                .iter()
                .map(|s| McpServerView {
                    managed: self.is_managed_mcp(&s.id),
                    id: s.id.clone(),
                    name: s.name.clone(),
                    transport: s.transport,
                    command: s.command.clone(),
                    args_text: s.args.join("\n"),
                    env_text: McpServer::format_pairs(&s.env, "="),
                    url: s.url.clone(),
                    headers_text: McpServer::format_pairs(&s.headers, ": "),
                    oauth_client_id: s.oauth_client_id.clone(),
                    oauth_callback_port: s.oauth_callback_port.map(|p| p.to_string()).unwrap_or_default(),
                    oauth_scopes: s.oauth_scopes.clone(),
                    used_by: self.state.bindings.iter().filter(|b| b.mcp_server_ids.contains(&s.id)).map(|b| folder_name(&b.path)).collect(),
                })
                .collect(),
            mcp_catalog: self.config.managed_mcp_servers_url.as_ref().map(|url| McpCatalogView {
                url: url.clone(),
                fetched_at: self.mcp_catalog.as_ref().map(|c| c.fetched_at),
                count: self.mcp_catalog.as_ref().map_or(0, |c| c.servers.len()),
                skipped: self.mcp_catalog.as_ref().map(|c| c.skipped.clone()).unwrap_or_default(),
                shadowed: self.shadowed_mcp_names(),
                problem: self.mcp_catalog_problem.clone(),
                loading: self.mcp_catalog_loading,
            }),
            claude_tools_enabled: self.config.claude_tools_enabled,
            tools: self.tools.clone(),
            installing_tool: self.installing_tool,
            claude_desktop: ClaudeDesktopView {
                profile_id: self.claude_desktop.profile_id.clone(),
                applied: self.claude_desktop.applied,
                managed: self.claude_desktop.managed,
                auto: self.claude_desktop.profile_id.as_deref().and_then(|id| self.state.profile(id)).is_some_and(|p| {
                    self.config.is_managed(&p.id) && p.claude_desktop.as_ref().is_some_and(|d| d.auto_apply)
                }),
                problem: self.desktop_problem.clone(),
                install_error: self.state.claude_desktop_install_error.clone(),
            },
            subscription: self.subscription.clone().filter(|_| self.has_subscription_profile()),
            office: self.office_view(),
            global_profile_id: self.state.global_binding.as_ref().map(|g| g.profile_id.clone()),
            global_health: self.global_health.as_ref().map(HealthView::from),
            global_findings: self.global_findings.clone(),
            user_settings_exists: Path::new(&user_settings_path).exists(),
            user_settings_path: abbreviate_home(&user_settings_path),
            user_settings_full_path: user_settings_path,
            available_backup: self.available_backup.as_ref().map(|b| BackupView { profiles: b.profiles.len(), projects: b.bindings.len() }),
            backup_problem: self.backup_problem.clone(),
            outdated_clis: self.outdated_clis.clone(),
            minimum_cli_version: self.minimum_cli_version(),
            available_update: self.available_update.as_ref().map(|u| UpdateView {
                version: u.version.clone(),
                prerelease: u.prerelease,
                page_url: u.page_url.clone(),
            }),
            installing_update: self.installing_update,
            whats_new: self.whats_new.clone(),
            keychain_keys: self.keychain_profiles.len(),
            notice: self.notice.clone(),
            error: self.error.clone(),
            allow_key_export: self.config.allow_key_export,
            update_check_enabled: self.config.update_check_enabled,
            beta_updates: self.beta_updates(),
            allowed_hosts: self.config.allowed_gateway_hosts.clone(),
            default_endpoint: self.config.default_endpoint.clone(),
            default_model_alias: self.config.default_model_alias.clone(),
            project_url: format!("https://github.com/{}", keyzapper_core::update::REPOSITORY),
            minimum_password_length: keyzapper_core::backup::MINIMUM_PASSWORD_LENGTH,
            vibrancy: self.vibrancy,
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `keyzapper-helper` next to the app executable (`KeyZapper.app/Contents/MacOS`, the install folder on Windows).
pub fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = format!("{}{}", helper::EXECUTABLE_NAME, std::env::consts::EXE_SUFFIX);
    let path = exe.parent()?.join(name);
    path.is_file().then_some(path)
}

/// Project status, the default profile and the settings check; computed outside the lock.
pub fn status_snapshot(state: &AppState, binder: &SettingsBinder) -> (HashMap<String, BindingStatus>, HashMap<String, bool>, Option<BindingHealth>, Vec<Finding>) {
    let global_values = state.global_binding.as_ref().map(|g| g.managed_values.clone()).unwrap_or_default();
    let mut statuses = HashMap::new();
    let mut mcp_active = HashMap::new();
    let mut global_health = None;
    for binding in &state.bindings {
        mcp_active.insert(binding.id.clone(), mcp::is_active(&binding.path, &binding.mcp_managed));
    }
    if !state.is_disabled() {
        for binding in &state.bindings {
            if let Some(profile) = state.profile(&binding.profile_id) {
                statuses.insert(binding.id.clone(), binder.inspect(binding, profile, &global_values));
            }
        }
        if let Some(global) = &state.global_binding {
            if let Some(profile) = state.profile(&global.profile_id) {
                global_health = Some(binder.inspect_global(global, profile));
            }
        }
    }
    let findings = keyzapper_core::audit::check(&binder.user_settings_path, &global_values);
    (statuses, mcp_active, global_health, findings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    platform: &'static str,
    language: &'static str,
    app_version: Option<String>,
    disabled: bool,
    profiles: Vec<ProfileView>,
    bindings: Vec<BindingView>,
    mcp_servers: Vec<McpServerView>,
    /// Present if IT set `ManagedMcpServersURL`.
    mcp_catalog: Option<McpCatalogView>,
    claude_tools_enabled: bool,
    tools: Option<Vec<ToolStatus>>,
    installing_tool: Option<Tool>,
    claude_desktop: ClaudeDesktopView,
    /// Plan and usage limits of the Claude subscription, shared by all subscription profiles.
    subscription: Option<subscription::Status>,
    office: OfficeView,
    global_profile_id: Option<String>,
    global_health: Option<HealthView>,
    global_findings: Vec<Finding>,
    user_settings_path: String,
    user_settings_full_path: String,
    user_settings_exists: bool,
    available_backup: Option<BackupView>,
    backup_problem: Option<String>,
    outdated_clis: Vec<String>,
    minimum_cli_version: String,
    available_update: Option<UpdateView>,
    installing_update: bool,
    whats_new: Vec<whats_new::Release>,
    keychain_keys: usize,
    notice: Option<String>,
    error: Option<String>,
    allow_key_export: bool,
    update_check_enabled: bool,
    /// Update checks include beta releases.
    beta_updates: bool,
    allowed_hosts: Vec<String>,
    default_endpoint: Option<String>,
    default_model_alias: Option<String>,
    project_url: String,
    minimum_password_length: usize,
    vibrancy: bool,
}

#[derive(Debug, PartialEq)]
pub struct TrayInfo {
    pub disabled: bool,
    pub profiles: Vec<TrayProfile>,
    pub projects: Vec<TrayProject>,
}

#[derive(Debug, PartialEq)]
pub struct TrayProfile {
    pub id: String,
    pub name: String,
    /// Key stored or signed in.
    pub usable: bool,
    /// Spend and limit as the gateway reported them, one per tenant (see `Model::budgets`).
    pub budgets: Vec<TrayBudget>,
}

#[derive(Debug, PartialEq)]
pub struct TrayBudget {
    pub tenant: Option<String>,
    pub spend: f64,
    pub max: Option<f64>,
}

#[derive(Debug, PartialEq)]
pub struct TrayProject {
    pub id: String,
    pub name: String,
    pub profile_id: String,
    pub healthy: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileView {
    id: String,
    sso: bool,
    /// Uses Claude Code's claude.ai login instead of a gateway.
    subscription: bool,
    oidc_issuer: Option<String>,
    oidc_client_id: Option<String>,
    oidc_scope: Option<String>,
    oidc_token_type: OidcTokenType,
    /// `loggedIn`, `loggedOut` or `expired` for SSO profiles.
    sso_state: Option<&'static str>,
    sso_expires_at: Option<i64>,
    sso_logging_in: bool,
    name: String,
    endpoint: String,
    model_alias: String,
    opus_model: Option<String>,
    sonnet_model: Option<String>,
    haiku_model: Option<String>,
    environment_text: String,
    managed: bool,
    /// IT sets Claude in Office / Claude Desktop's sign-in for this managed profile; the user cannot change them then.
    office_managed: bool,
    claude_desktop_managed: bool,
    has_key: bool,
    key_hint: Option<String>,
    /// One per tenant of an SSO profile's projects, else at most one; empty while unknown.
    budgets: Vec<BudgetView>,
    killers: Vec<String>,
    /// Tenants the gateway lists for this profile (`<tenant>/<model>` IDs), sorted; empty until the models are loaded.
    tenants: Vec<String>,
    /// Claude in Office settings of the profile, None if never configured.
    office: Option<OfficeClient>,
    /// Claude Desktop's own sign-in, None = via the helper.
    claude_desktop: Option<DesktopSso>,
}

/// Claude in Office on this machine: `state` is `off`, `applied` or `error`; `apps` are the Office apps it is registered
/// for; `profile_id` the profile it comes from (None while KeyZapper is deactivated or no profile has Office switched on).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OfficeView {
    #[serde(flatten)]
    status: OfficeStatus,
    profile_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BudgetView {
    /// None = the profile itself (its own models), not a project's tenant.
    tenant: Option<String>,
    spend: f64,
    max_budget: Option<f64>,
    remaining: Option<f64>,
    reset_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingView {
    id: String,
    path: String,
    display_path: String,
    name: String,
    profile_id: String,
    pooled: bool,
    /// Tenant whose models the project uses; None = as in the profile.
    tenant: Option<String>,
    mcp_server_ids: Vec<String>,
    mcp_names: Vec<String>,
    /// False if `.mcp.json` no longer contains what the app wrote.
    mcp_active: bool,
    status: Option<StatusView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpServerView {
    id: String,
    /// From the central list: IT changes it, the app only switches it on per project.
    managed: bool,
    name: String,
    transport: McpTransport,
    command: String,
    args_text: String,
    env_text: String,
    url: String,
    headers_text: String,
    oauth_client_id: String,
    oauth_callback_port: String,
    oauth_scopes: String,
    /// Names of the projects using this server.
    used_by: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpCatalogView {
    url: String,
    /// Unix time of the last successful fetch.
    fetched_at: Option<i64>,
    count: usize,
    skipped: Vec<String>,
    shadowed: Vec<String>,
    problem: Option<String>,
    loading: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeDesktopView {
    profile_id: Option<String>,
    applied: bool,
    managed: bool,
    /// Set up by IT without a click (`AutoApply`).
    auto: bool,
    /// Why the setup IT asks for is not written.
    problem: Option<String>,
    /// Why the background installation failed.
    install_error: Option<String>,
}

#[derive(Serialize)]
struct StatusView {
    health: HealthView,
    conflicts: Vec<ConflictView>,
}

#[derive(Serialize)]
struct ConflictView {
    blocking: bool,
    message: String,
}

#[derive(Serialize)]
struct HealthView {
    /// `active`, `notApplied`, `drifted` or `folderMissing`.
    state: &'static str,
    keys: Vec<String>,
}

impl From<&BindingHealth> for HealthView {
    fn from(health: &BindingHealth) -> Self {
        match health {
            BindingHealth::Active => HealthView { state: "active", keys: vec![] },
            BindingHealth::NotApplied => HealthView { state: "notApplied", keys: vec![] },
            BindingHealth::Drifted(keys) => HealthView { state: "drifted", keys: keys.clone() },
            BindingHealth::FolderMissing => HealthView { state: "folderMissing", keys: vec![] },
        }
    }
}

#[derive(Serialize)]
struct BackupView {
    profiles: usize,
    projects: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateView {
    version: String,
    prerelease: bool,
    page_url: String,
}

/// Office folders are real folders on macOS; on Windows the registry is used instead (see `claude_office`'s own test).
#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use keyzapper_core::claude_office::OfficeState;

    const TEMPLATE: &str = include_str!("../../core/tests/fixtures/office-manifest.xml");
    const ID: &str = "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c";

    fn office() -> OfficeClient {
        OfficeClient {
            enabled: true,
            addin_id: ID.into(),
            gateway_url: "https://llm-gateway.example/api/x/desktop-gateway/office".into(),
            entra_client_id: "11111111-2222-4333-8444-555555555555".into(),
            entra_scope: "api://11111111-2222-4333-8444-555555555555/access_as_user".into(),
            disabled_features: vec![],
        }
    }

    /// A model that keeps its files and Office folders below `root` and has no managed settings: it reads nothing from this
    /// machine's Intune values and writes nothing outside `root`.
    fn model_in(root: &Path) -> Model {
        let mut model = Model::load_from(None, ManagedConfig::default(), root.join("data"));
        assert!(model.backup_dir.is_none(), "an empty config has no backup folder");
        model.office_sideload = Sideload {
            containers_dir: root.join("Containers"),
            applications_dir: root.join("Applications"),
            manifest_dir: root.join("office"),
            registry_key: r"SOFTWARE\KeyZapperTest".into(),
        };
        model
    }

    /// Plans with `TEMPLATE` as the fetched one and runs the job, like `sync_office` in main.rs.
    fn sync(model: &mut Model, retry: bool) -> Option<SyncAction> {
        sync_checked(model, retry, |_| {})
    }

    /// Like `sync`; `at_checkpoint` runs right after the intended record was saved, before any manifest is written.
    fn sync_checked(model: &mut Model, retry: bool, mut at_checkpoint: impl FnMut(&Model)) -> Option<SyncAction> {
        let job = model.plan_office(Some(Ok(TEMPLATE.into())), retry)?;
        let sideload = model.office_sideload.clone();
        let outcome = job.run(&sideload, &mut |interim| {
            let saved = model.record_office(interim);
            at_checkpoint(model);
            saved
        });
        let action = job.action.clone();
        model.finish_office(&job, outcome);
        Some(action)
    }

    /// The model IDs of a plain LiteLLM gateway: provider prefixes, not tenants.
    fn prefixed_models() -> Vec<String> {
        ["bedrock/claude-sonnet-5-5", "bedrock/claude-opus-5", "azure/gpt-5"].map(String::from).to_vec()
    }

    fn sso_profile(name: &str) -> Profile {
        let mut profile = Profile::new(new_id(), name.into(), "https://llm-gateway.example/api/x/desktop-gateway/code".into(), String::new());
        profile.auth_type = AuthType::Oidc;
        profile.oidc_issuer = Some("https://login.example.test/00000000-0000-4000-8000-000000000000/v2.0".into());
        profile.oidc_client_id = Some("11111111-2222-4333-8444-555555555555".into());
        profile
    }

    fn model_with_binder(root: &Path) -> Model {
        let mut model = model_in(root);
        model.binder = Some(SettingsBinder {
            helper_path: root.join("keyzapper-helper").to_string_lossy().into_owned(),
            managed_settings_paths: vec![],
            user_settings_path: root.join("user-settings.json").to_string_lossy().into_owned(),
        });
        model
    }

    #[test]
    fn only_an_sso_profile_offers_tenants_and_takes_a_tenant_choice() {
        let root = std::env::temp_dir().join(format!("kz-tenants-model-{}", new_id()));
        let mut model = model_with_binder(&root);
        let key = Profile::new(new_id(), "Key".into(), "https://litellm.example.test".into(), String::new());
        let sso = sso_profile("Company");
        assert!(model.save_profile(key.clone(), Some("sk-test".into())) && model.save_profile(sso.clone(), None));
        // Whatever the gateway lists is cached for both, as if loaded.
        for id in [&key.id, &sso.id] {
            model.gateway_models.insert(id.clone(), prefixed_models());
        }
        // A key profile has no tenants: `bedrock/…` and `azure/…` are providers of one gateway, not accounts to choose from.
        let tenants = |model: &Model, id: &str| model.view().profiles.into_iter().find(|p| p.id == id).unwrap().tenants;
        assert!(tenants(&model, &key.id).is_empty());
        assert_eq!(tenants(&model, &sso.id), ["azure", "bedrock"]);

        // The project of a key profile stays on the profile's own models, whatever is asked for.
        let mut projects = vec![];
        for profile in [&key, &sso] {
            let folder = root.join(format!("project-{}", profile.name));
            std::fs::create_dir_all(&folder).unwrap();
            model.bind(&folder.to_string_lossy(), &profile.id);
            projects.push((model.state.bindings.iter().find(|b| b.profile_id == profile.id).cloned().expect("bound"), folder));
        }
        let (key_project, key_folder) = &projects[0];
        let settings_of_key_project = std::fs::read_to_string(key_folder.join(".claude/settings.local.json")).unwrap();
        model.error = None;
        model.set_binding_tenant(&key_project.id, Some("bedrock".into()));
        assert!(model.error.is_none(), "{:?}", model.error);
        assert!(model.state.bindings.iter().find(|b| b.id == key_project.id).unwrap().models.is_none());
        assert_eq!(std::fs::read_to_string(key_folder.join(".claude/settings.local.json")).unwrap(), settings_of_key_project);

        // An SSO profile's project takes the tenant.
        let (sso_project, sso_folder) = &projects[1];
        model.set_binding_tenant(&sso_project.id, Some("bedrock".into()));
        assert!(model.error.is_none(), "{:?}", model.error);
        let chosen = model.state.bindings.iter().find(|b| b.id == sso_project.id).unwrap().models.clone();
        assert_eq!(chosen.map(|m| m.tenant), Some("bedrock".to_string()));
        assert!(std::fs::read_to_string(sso_folder.join(".claude/settings.local.json")).unwrap().contains("bedrock/claude-sonnet-5-5"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A model with the managed profiles `profiles` (JSON as in Intune), a binder and its own Claude Desktop library below `root`.
    fn managed_model(root: &Path, profiles: serde_json::Value) -> Model {
        let config = ManagedConfig::from_values(serde_json::json!({ "ManagedProfiles": profiles }).as_object().unwrap());
        let mut model = Model::load_from(None, config, root.join("data"));
        model.binder = Some(SettingsBinder {
            helper_path: root.join("keyzapper-helper").to_string_lossy().into_owned(),
            managed_settings_paths: vec![],
            user_settings_path: root.join("user-settings.json").to_string_lossy().into_owned(),
        });
        model.desktop_library = root.join("configLibrary");
        model
    }

    #[test]
    fn claude_desktop_is_set_up_without_a_click_and_the_earlier_configuration_comes_back() {
        let root = std::env::temp_dir().join(format!("kz-desktop-auto-{}", new_id()));
        let block = |auto: bool| {
            serde_json::json!({"OIDCIssuer": "https://login.example.test/t/v2.0", "OIDCClientID": "c1", "AutoApply": auto,
                "Config": {"disableDeploymentModeChooser": true}})
        };
        let profile = |name: &str, auto: bool| serde_json::json!({"Name": name, "Endpoint": "https://llm-gateway.example/code", "ClaudeDesktop": block(auto)});
        let mut model = managed_model(&root, serde_json::json!([profile("Firma", true), profile("Andere", false)]));
        let library = model.desktop_library.clone();
        let entry = library.join(format!("{}.json", claude_desktop::ENTRY_ID));
        // The user had applied a configuration of their own.
        let own = "11111111-0000-4000-8000-000000000001";
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join(format!("{own}.json")), "{}").unwrap();
        std::fs::write(library.join("_meta.json"), serde_json::json!({"appliedId": own, "entries": [{"id": own, "name": "Eigene"}]}).to_string()).unwrap();

        model.auto_apply_claude_desktop();
        assert_eq!(claude_desktop::applied_id(&library).as_deref(), Some(claude_desktop::ENTRY_ID));
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&entry).unwrap()).unwrap();
        assert_eq!((written["disableDeploymentModeChooser"].clone(), written["inferenceCredentialKind"].clone()), (true.into(), "external-idp".into()));
        let view = model.view().claude_desktop;
        assert!(view.auto && view.applied && view.problem.is_none());

        // A deleted entry is written again; an unchanged one is left alone.
        std::fs::remove_file(&entry).unwrap();
        model.auto_apply_claude_desktop();
        assert!(entry.is_file());
        let modified = std::fs::metadata(&entry).unwrap().modified().unwrap();
        model.auto_apply_claude_desktop();
        assert_eq!(std::fs::metadata(&entry).unwrap().modified().unwrap(), modified);

        // Deactivating brings the user's own configuration back; activating sets the organisation's up again.
        model.set_disabled(true);
        assert!(!entry.exists());
        assert_eq!(claude_desktop::applied_id(&library).as_deref(), Some(own));
        model.auto_apply_claude_desktop();
        assert!(!entry.exists(), "nothing is written while KeyZapper is deactivated");
        model.set_disabled(false);
        assert_eq!(claude_desktop::applied_id(&library).as_deref(), Some(claude_desktop::ENTRY_ID));
        let _ = std::fs::remove_dir_all(&root);

        // Two profiles asking for it: none is set up, and the settings say why.
        let root = std::env::temp_dir().join(format!("kz-desktop-auto-two-{}", new_id()));
        let mut model = managed_model(&root, serde_json::json!([profile("Firma", true), profile("Andere", true)]));
        model.error = None;
        model.auto_apply_claude_desktop();
        assert!(!model.desktop_library.join(format!("{}.json", claude_desktop::ENTRY_ID)).exists());
        assert!(model.view().claude_desktop.problem.is_some() && model.error.is_none(), "{:?}", model.error);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_managed_profile_takes_the_users_claude_desktop_sign_in_unless_it_sets_one() {
        let root = std::env::temp_dir().join(format!("kz-managed-own-desktop-{}", new_id()));
        let managed = |with_block: bool| {
            let mut profile = serde_json::json!({"Name": "Firma", "Endpoint": "https://llm-gateway.example/code"});
            if with_block {
                profile["ClaudeDesktop"] = serde_json::json!({"OIDCIssuer": "https://login.example.test/t/v2.0", "OIDCClientID": "it-client"});
            }
            serde_json::json!([profile])
        };
        let mut model = managed_model(&root, managed(false));
        let id = model.state.profiles[0].id.clone();
        let input = |enabled: bool| ProfileInput {
            id: Some(id.clone()), name: "Umbenannt".into(), endpoint: "https://llm-gateway.example/code".into(), model_alias: String::new(),
            opus_model: String::new(), sonnet_model: String::new(), haiku_model: String::new(), environment_text: String::new(), key: String::new(),
            auth_type: AuthType::ApiKey, oidc_issuer: String::new(), oidc_client_id: String::new(), oidc_scope: String::new(), oidc_token_type: OidcTokenType::Access,
            office: None,
            claude_desktop: Some(DesktopSsoInput { enabled, gateway_url: String::new(), oidc_issuer: "https://login.example.test/t/v2.0".into(), oidc_client_id: "own-client".into(),
                scopes: String::new(), bearer_token_type: BearerTokenType::IdToken, credential_kind: CredentialKind::Interactive }),
        };
        // IT sets no block: the user's own is taken, the rest of the profile stays IT's.
        assert!(model.save_profile_input(input(true)), "{:?}", model.error);
        let profile = model.state.profiles[0].clone();
        assert_eq!((profile.name.as_str(), profile.claude_desktop.as_ref().map(|d| d.oidc_client_id.as_str())), ("Firma", Some("own-client")));
        let view = model.view();
        assert!(view.profiles[0].managed && !view.profiles[0].claude_desktop_managed);
        // It survives the sync with Intune and a restart.
        model.sync_managed_profiles();
        assert_eq!(model.state.profiles[0].claude_desktop.as_ref().map(|d| d.oidc_client_id.as_str()), Some("own-client"));
        let reloaded = managed_model(&root, managed(false));
        assert_eq!(reloaded.state.profiles[0].claude_desktop.as_ref().map(|d| d.oidc_client_id.as_str()), Some("own-client"));
        // Switched off again, it goes.
        assert!(model.save_profile_input(input(false)), "{:?}", model.error);
        assert!(model.state.profiles[0].claude_desktop.is_none());

        // IT sets the block: it wins, and the editor cannot change or remove it.
        let mut model = managed_model(&root, managed(true));
        assert_eq!(model.state.profiles[0].claude_desktop.as_ref().map(|d| d.oidc_client_id.as_str()), Some("it-client"));
        assert!(model.view().profiles[0].claude_desktop_managed);
        assert!(model.save_profile_input(input(false)), "{:?}", model.error);
        assert_eq!(model.state.profiles[0].claude_desktop.as_ref().map(|d| d.oidc_client_id.as_str()), Some("it-client"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_beta_switch_is_kept_and_defaults_to_on_only_for_an_installed_beta() {
        let root = std::env::temp_dir().join(format!("kz-beta-{}", new_id()));
        let load = |version: &str| Model::load_from(Some(version.into()), ManagedConfig::default(), root.join("data"));
        assert!(!load("1.5.0").beta_updates());
        assert!(load("1.5.0-beta.1").beta_updates(), "testers get the next beta");

        // Switched off, an offered beta goes; the choice is saved and wins over the installed version.
        let mut model = load("1.5.0-beta.1");
        let beta = ReleaseInfo { version: "1.5.0-beta.2".into(), prerelease: true, page_url: String::new(), package_url: None, package_sha256: None, signature_url: None };
        model.available_update = Some(beta.clone());
        model.set_beta_updates(false);
        assert!(model.available_update.is_none() && !model.beta_updates());
        assert!(!load("1.5.0-beta.1").beta_updates());
        // Switched on, it stays on for a stable version too.
        model.set_beta_updates(true);
        assert!(load("1.5.0").beta_updates());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tenant_choice_follows_the_profiles_models() {
        let root = std::env::temp_dir().join(format!("kz-tenant-follow-{}", new_id()));
        let mut model = model_with_binder(&root);
        let mut profile = sso_profile("Company");
        profile.sonnet_model = Some("claude-sonnet-5".into());
        assert!(model.save_profile(profile.clone(), None));
        let models: Vec<String> = ["acme/claude-sonnet-5", "acme/claude-sonnet-5-5", "globex/claude-sonnet-5"].map(String::from).to_vec();
        model.gateway_models.insert(profile.id.clone(), models.clone());
        let folder = root.join("project");
        std::fs::create_dir_all(&folder).unwrap();
        model.bind(&folder.to_string_lossy(), &profile.id);
        let binding_id = model.state.bindings[0].id.clone();
        model.set_binding_tenant(&binding_id, Some("acme".into()));
        let settings = || std::fs::read_to_string(folder.join(".claude/settings.local.json")).unwrap();
        assert!(settings().contains("\"acme/claude-sonnet-5\""), "{}", settings());

        // The profile moves to a newer Sonnet: the project follows, within its tenant.
        profile.sonnet_model = Some("claude-sonnet-5-5".into());
        assert!(model.save_profile(profile.clone(), None));
        assert!(settings().contains("\"acme/claude-sonnet-5-5\""), "{}", settings());
        assert_eq!(model.state.bindings[0].models.as_ref().map(|m| m.tenant.as_str()), Some("acme"));

        // Without the gateway's list the stored models stay; once it is loaded, the project follows.
        model.gateway_models.remove(&profile.id);
        profile.sonnet_model = Some("claude-sonnet-5".into());
        assert!(model.save_profile(profile.clone(), None));
        assert!(settings().contains("\"acme/claude-sonnet-5-5\""));
        model.gateway_models.insert(profile.id.clone(), models);
        model.follow_tenants(&profile.id);
        assert!(settings().contains("\"acme/claude-sonnet-5\""), "{}", settings());

        // A profile that is no longer SSO has no tenants: the project returns to the profile's own models.
        profile.auth_type = AuthType::ApiKey;
        assert!(model.save_profile(profile.clone(), Some("sk-test".into())));
        assert!(model.state.bindings[0].models.is_none());
        assert!(settings().contains("\"claude-sonnet-5\"") && !settings().contains("acme/"), "{}", settings());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_profile_replaced_by_an_import_drops_the_model_list_of_its_old_gateway() {
        let root = std::env::temp_dir().join(format!("kz-import-model-{}", new_id()));
        let mut model = model_with_binder(&root);
        let (changing, unchanged) = (sso_profile("Changing"), sso_profile("Unchanged"));
        assert!(model.save_profile(changing.clone(), None) && model.save_profile(unchanged.clone(), None));
        for id in [&changing.id, &unchanged.id] {
            model.gateway_models.insert(id.clone(), prefixed_models());
        }
        let mut replaced = changing.clone();
        replaced.endpoint = "https://other-gateway.example/api/x/desktop-gateway/code".into();
        let payload = BackupPayload {
            created_at: "2026-10-20T00:00:00Z".into(),
            app_version: None,
            state: AppState { profiles: vec![replaced.clone(), unchanged.clone()], ..AppState::default() },
            keys: BTreeMap::new(),
        };
        assert!(model.import_payload(payload));
        assert_eq!(model.state.profile(&changing.id).map(|p| p.endpoint.as_str()), Some(replaced.endpoint.as_str()));
        assert!(!model.gateway_models.contains_key(&changing.id), "the list belongs to the old gateway; the next refresh loads the new one");
        assert!(model.gateway_models.contains_key(&unchanged.id), "a profile the import left as it was keeps its list");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_in_office_follows_the_profile_and_a_failed_registration_is_not_repeated_on_every_start() {
        let root = std::env::temp_dir().join(format!("kz-office-model-{}", new_id()));
        let wef = |bundle: &str| root.join("Containers").join(bundle).join("Data/Documents/wef");
        let word_manifest = wef("com.microsoft.Word").join(format!("{ID}.manifest.xml"));
        for app in ["Microsoft Excel.app", "Microsoft Word.app", "Microsoft PowerPoint.app"] {
            std::fs::create_dir_all(root.join("Applications").join(app)).unwrap();
        }
        // PowerPoint was never started: it has no container, and KeyZapper does not create one.
        std::fs::create_dir_all(root.join("Containers/com.microsoft.Word")).unwrap();
        // Excel's wef folder is blocked by a file: Excel fails, Word works.
        std::fs::create_dir_all(wef("com.microsoft.Excel").parent().unwrap()).unwrap();
        std::fs::write(wef("com.microsoft.Excel"), "not a folder").unwrap();

        let mut model = model_in(&root);
        let mut profile = Profile::new(new_id(), "Own".into(), "https://litellm.example.test".into(), String::new());
        profile.office = Some(office());
        assert!(model.save_profile(profile.clone(), Some("sk-test".into())));
        assert_eq!(model.office_view().status.state, OfficeState::Off);

        // The first run registers; Excel fails, and the failure is recorded together with what was intended.
        assert_eq!(sync(&mut model, false), Some(SyncAction::Apply));
        assert!(word_manifest.exists());
        assert!(!root.join("Containers/com.microsoft.Powerpoint").exists(), "no container is created for an app never started");
        let status = model.office_view().status;
        assert_eq!((status.state, status.apps.clone()), (OfficeState::Error, vec!["excel".to_string(), "word".to_string()]));
        assert!(status.message.is_some() && model.office_failed());
        assert_eq!(model.office_view().profile_id.as_deref(), Some(profile.id.as_str()));

        // A new start (state read from disk, no retry requested) writes nothing and still shows the error.
        let mut model = model_in(&root);
        assert!(model.state.office_error.is_some() && model.state.office_applied.is_some());
        std::fs::remove_file(&word_manifest).unwrap();
        assert_eq!(sync(&mut model, false), None);
        assert!(!word_manifest.exists());
        assert_eq!(model.office_view().status.state, OfficeState::Error);

        // A request of the user tries again; with Excel fixed everything is registered and the error is gone.
        std::fs::remove_file(wef("com.microsoft.Excel")).unwrap();
        assert_eq!(sync(&mut model, true), Some(SyncAction::Apply));
        assert!(word_manifest.exists() && wef("com.microsoft.Excel").join(format!("{ID}.manifest.xml")).exists());
        assert_eq!(model.office_view().status.state, OfficeState::Applied);
        assert!(!model.office_failed());
        assert_eq!(sync(&mut model, true), None, "nothing changed, nothing is written");

        // Unusable settings write nothing and leave the registered manifest in place.
        model.state.profiles[0].office.as_mut().unwrap().gateway_url = "https://localhost/office".into();
        assert_eq!(sync(&mut model, false), None);
        assert!(word_manifest.exists());
        let status = model.office_view().status;
        assert_eq!((status.state, status.apps.len()), (OfficeState::Error, 2));
        // A gateway the company policy does not allow is removed: an add-in must not keep using it.
        model.state.profiles[0].office = Some(office());
        model.config.allowed_gateway_hosts = vec!["other.example".into()];
        assert_eq!(sync(&mut model, false), Some(SyncAction::Remove { addin_id: ID.into() }));
        assert!(!word_manifest.exists());
        assert_eq!(model.office_view().status.state, OfficeState::Error);
        model.config = ManagedConfig::default();
        assert_eq!(sync(&mut model, false), Some(SyncAction::Apply));
        assert!(word_manifest.exists());

        // Deactivating removes the manifests; only KeyZapper's own files go.
        std::fs::write(wef("com.microsoft.Word").join("foreign.manifest.xml"), "x").unwrap();
        model.state.disabled = Some(true);
        assert_eq!(sync(&mut model, false), Some(SyncAction::Remove { addin_id: ID.into() }));
        assert!(!word_manifest.exists() && wef("com.microsoft.Word").join("foreign.manifest.xml").exists());
        assert_eq!((model.office_view().status.state, model.office_view().profile_id), (OfficeState::Off, None));
        // Activating registers again. The record is on disk before the first manifest is written, so a crash in between
        // cannot leave a manifest the record does not know.
        model.state.disabled = None;
        let state_file = root.join("data/state.json");
        let mut checked = false;
        assert_eq!(
            sync_checked(&mut model, false, |_| {
                let on_disk = MetadataStore::new(state_file.clone(), true).load().unwrap();
                assert_eq!(on_disk.office_applied.as_ref().map(|a| a.addin_id.as_str()), Some(ID));
                assert!(on_disk.office_error.is_some(), "marked unfinished until the run is done");
                assert!(!word_manifest.exists());
                checked = true;
            }),
            Some(SyncAction::Apply)
        );
        assert!(checked && word_manifest.exists());
        assert_eq!(model.office_view().status.state, OfficeState::Applied);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_editor_input_of_claude_desktop_carries_the_credential_kind() {
        let input = |enabled: bool| {
            serde_json::from_value::<DesktopSsoInput>(serde_json::json!({
                "enabled": enabled, "gatewayUrl": "", "oidcIssuer": " https://login.example.com/t/v2.0 ", "oidcClientId": "c1",
                "scopes": "", "bearerTokenType": "id_token", "credentialKind": "interactive",
            }))
            .unwrap()
        };
        let sso = input(true).into_sso().unwrap();
        assert_eq!(sso.credential_kind, CredentialKind::Interactive);
        assert_eq!(sso.oidc_issuer, "https://login.example.com/t/v2.0");
        assert!(input(false).into_sso().is_none());
    }
}
