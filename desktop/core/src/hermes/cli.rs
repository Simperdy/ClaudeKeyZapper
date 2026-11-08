use super::{ConfigAccess, ConfigSnapshot, HermesError, HermesResult, HermesTarget, StoredValue};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::{Path, PathBuf}, process::{Command, Stdio}, thread, time::{Duration, Instant}};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(test)]
pub struct CompatibilityEvidence { executable: PathBuf, home: PathBuf, supported: bool }
#[cfg(test)]
impl CompatibilityEvidence {
    fn validated(target: &HermesTarget, supported: bool) -> Self {
        Self { executable: target.executable.clone(), home: target.home.clone(), supported }
    }
}

const OUTPUT_LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
const COMPATIBILITY_FIXTURE: &str = include_str!("../../tests/fixtures/hermes-compatibility.json");
const SAFE_PROBE: &str = include_str!("compatibility_probe.py");
const REQUIRED_CHECKS: &[&str] = &[
    "identity", "safe_local_capability_diagnostic", "native_parser", "concurrent_real_helper",
    "same_live_source_real_helper_rotation", "resolved_fallback_and_tools_disabled",
    "native_json_sse_tool_parser_no_execution", "native_cli_keeps_json_scalar_quotes_literal",
    "installed_python_cli_roundtrip_supplemental", "installed_python_cli_unset_stored_absence",
    "selected_launcher_roundtrip", "selected_launcher_unset_stored_absence", "selected_launcher_chat",
];

fn evidence_is_complete(build: &Value) -> bool {
    let complete_checks = |value: &Value| value.as_object().is_some_and(|checks|
        checks.len() == REQUIRED_CHECKS.len() && REQUIRED_CHECKS.iter().all(|name| checks.get(*name).and_then(Value::as_bool) == Some(true)));
    let native = &build["nativeResult"];
    build.get("passed").and_then(Value::as_bool) == Some(true)
        && build.get("fullPass").and_then(Value::as_bool) == Some(true)
        && build["sdkJsonSseToolParsing"] == true && build["acceptanceExit"] == 0
        && complete_checks(&build["checks"]) && complete_checks(&native["checks"])
        && native["harness_status"] == "passed" && native["full_pass"] == true
        && native["helper_fingerprint_verified"] == true && native["anthropic_sdk_available"] == true
        && native["launcher_version_exit"] == 0 && native["chat_exit"] == 0
        && native["gateway_stop_exit"] == 0 && native["unexpected_http_count"] == 0
        && native["blockers"].as_array().is_some_and(Vec::is_empty)
        && native["unexpected_routes"].as_array().is_some_and(Vec::is_empty)
        && native["local_diagnostic"]["passed"] == true && native["local_diagnostic"]["sdkJsonSseToolParsing"] == true
        && [("versionLine", "version_line"), ("entrypointSha256", "entrypoint_sha256"), ("pythonVersion", "python")]
            .iter().all(|(outer, inner)| build[*outer].is_string() && build[*outer] == native[*inner])
        && ["sourceTreeSha256", "pythonVersion", "anthropicVersion"].iter()
            .all(|key| build[*key].is_string() && build[*key] == native["local_diagnostic"][*key])
}
fn evidence_matches(build: &Value, version: &str, entrypoint: &str, source: &str, python: &str, sdk: &str) -> bool {
    evidence_is_complete(build)
        && build.get("versionLine").and_then(Value::as_str) == Some(version)
        && build.get("entrypointSha256").and_then(Value::as_str) == Some(entrypoint)
        && build.get("sourceTreeSha256").and_then(Value::as_str) == Some(source)
        && build.get("pythonVersion").and_then(Value::as_str) == Some(python)
        && build.get("anthropicVersion").and_then(Value::as_str) == Some(sdk)
        && build.pointer("/nativeResult/full_pass").and_then(Value::as_bool) == Some(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability { Supported, Unknown, Unsupported }
pub struct HermesCli {
    #[cfg(test)]
    evidence: Option<CompatibilityEvidence>,
}
impl HermesCli {
    /// Read-only installation-bound verification; never starts a chat or gateway.
    pub fn capability(&self, target: &HermesTarget) -> Capability {
        if self.verify(target).is_ok() { Capability::Supported } else { Capability::Unsupported }
    }
    fn verify(&self, target: &HermesTarget) -> HermesResult<()> {
        let fixture: Value = serde_json::from_str(COMPATIBILITY_FIXTURE).map_err(|_| HermesError::Unsupported)?;
        verify_installation(target, &fixture, |command| run_command(command, |child| child.try_wait()))
    }
    pub fn new() -> Self { Self {
        #[cfg(test)]
        evidence: None,
    } }
    #[cfg(test)]
    fn with_evidence(evidence: CompatibilityEvidence) -> Self { Self { evidence: Some(evidence) } }
    fn check_write(&self, target: &HermesTarget) -> HermesResult<()> {
        #[cfg(test)]
        if let Some(e) = &self.evidence { return match e {
            e if e.supported && e.executable == target.executable && e.home == target.home => Ok(()),
            _ => Err(HermesError::Unsupported),
        }; }
        // Do not trust a previous UI bool: rerun the exact fingerprint and diagnostic.
        self.verify(target).map_err(|_| HermesError::Unsupported)
    }
    fn execute(&self, target: &HermesTarget, args: &[String]) -> HermesResult<Vec<u8>> {
        self.execute_with_wait(target, args, |child| child.try_wait())
    }
    fn execute_with_wait(
        &self,
        target: &HermesTarget,
        args: &[String],
        try_wait: impl FnMut(&mut std::process::Child) -> std::io::Result<Option<std::process::ExitStatus>>,
    ) -> HermesResult<Vec<u8>> {
        let scratch = ProbeHome::new()?;
        let mut command = Command::new(&target.executable);
        command.args(args).env_clear().current_dir(&scratch.0)
            .env("HERMES_HOME", &target.home).env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", &scratch.0).env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONPYCACHEPREFIX", scratch.0.join("bytecode"))
            .env("LANG", "C.UTF-8").env("NO_COLOR", "1");
        // Keep explicit HOME semantics, but do not forward tokens, profiles,
        // PYTHONPATH, proxies or arbitrary caller environment to configuration CLI.
        if let Some(home) = std::env::var_os("HOME") { command.env("HOME", home); }
        run_command(command, try_wait)
    }
    fn path(&self, target: &HermesTarget) -> HermesResult<PathBuf> {
        let bytes = self.execute(target, &["config".into(), "path".into()])?;
        let text = std::str::from_utf8(&bytes).map_err(|_| HermesError::InvalidConfig)?.trim();
        if text.is_empty() { return Err(HermesError::InvalidConfig); }
        let path = PathBuf::from(text);
        let home = fs::canonicalize(&target.home).map_err(|_| HermesError::WrongTarget)?;
        let resolved = if path.exists() { fs::canonicalize(&path).map_err(|_| HermesError::WrongTarget)? }
            else { let parent = path.parent().ok_or(HermesError::WrongTarget)?; fs::canonicalize(parent).map_err(|_| HermesError::WrongTarget)?.join(path.file_name().ok_or(HermesError::WrongTarget)?) };
        if !resolved.starts_with(&home) { return Err(HermesError::WrongTarget); }
        Ok(resolved)
    }
    fn run_write(&self, target: &HermesTarget, args: Vec<String>) -> HermesResult<()> { self.execute(target, &args).map(|_| ()) }
}
impl Default for HermesCli { fn default() -> Self { Self::new() } }

/// Private inputs only: production always supplies the embedded acceptance manifest.
fn verify_installation(target: &HermesTarget, fixture: &Value, mut run: impl FnMut(Command) -> HermesResult<Vec<u8>>) -> HermesResult<()> {
    let unsupported = || HermesError::Unsupported;
    if !cfg!(unix) || !target.executable.is_absolute() || !target.home.is_absolute() || fixture["schemaVersion"] != 1 {
        return Err(unsupported());
    }
    // Reject unknown launchers before executing them, including version-only shims.
    let script = fs::read(&target.executable).map_err(|_| unsupported())?;
    if script.len() > 4096 { return Err(unsupported()); }
    let entrypoint = format!("{:x}", Sha256::digest(&script));
    // Identical script bytes are not an installation identity: the console script's own
    // directory becomes sys.path[0], so the selected runtime must be the very entrypoint
    // the accepted build recorded. This is checked before ANY --version/probe execution.
    let selected = fs::canonicalize(&target.executable).map_err(|_| unsupported())?;
    let builds = fixture["supportedBuilds"].as_array().ok_or_else(unsupported)?;
    let candidates: Vec<_> = builds.iter().filter(|build| {
        let recorded = build["nativeResult"]["selected_executable"].as_str().map(PathBuf::from);
        evidence_is_complete(build)
            && build["entrypointSha256"].as_str() == Some(&entrypoint)
            && recorded.as_deref() == Some(selected.as_path())
            && recorded.is_some_and(|path| fs::canonicalize(path).ok().as_deref() == Some(selected.as_path()))
    }).collect();
    if candidates.is_empty() { return Err(unsupported()); }
    // Even --version imports the console script's modules. Prevalidate the public
    // installation root recorded by actual acceptance before executing ANY CLI.
    // A relocated installation needs its own observed evidence, not path guessing.
    let verified_roots: Vec<_> = candidates.iter().filter_map(|build| {
        let root = PathBuf::from(build["nativeResult"]["source_root"].as_str()?);
        if !root.is_absolute() || fs::canonicalize(&root).ok()? != root
            || [".env", ".op.env"].iter().any(|name| root.join(name).symlink_metadata().is_ok()) { return None; }
        let tree = public_source_fingerprint(&root, fixture).ok()?;
        (build["sourceTreeSha256"].as_str() == Some(&tree)).then_some(root)
    }).collect();
    if verified_roots.is_empty() { return Err(unsupported()); }
    let script = std::str::from_utf8(&script).map_err(|_| unsupported())?;
    let python = PathBuf::from(script.lines().next().and_then(|line| line.strip_prefix("#!")).ok_or_else(unsupported)?);
    if !python.is_absolute() || !python.is_file() || python.to_string_lossy().chars().any(char::is_whitespace)
        || !script.contains("from hermes_cli.main import main") { return Err(unsupported()); }
    let scratch = ProbeHome::new()?;
    let command = |program: &Path, args: &[&str]| {
        let mut command = Command::new(program);
        command.args(args).env_clear().current_dir(&scratch.0)
            .env("HOME", scratch.0.join("user")).env("HERMES_HOME", scratch.0.join("hermes"))
            .env("TMPDIR", &scratch.0).env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONPYCACHEPREFIX", scratch.0.join("bytecode"))
            .env("PATH", format!("{}:/usr/bin:/bin", python.parent().unwrap_or(Path::new("/usr/bin")).display()))
            .env("LANG", "C.UTF-8").env("NO_COLOR", "1");
        command
    };
    let version_bytes = run(command(&target.executable, &["--version"]))?;
    let version = std::str::from_utf8(&version_bytes).map_err(|_| unsupported())?;
    let version_line = version.lines().next().ok_or_else(unsupported)?;
    let roots: Vec<_> = version.lines().filter_map(|line| line.strip_prefix("Install directory: ")).collect();
    if roots.len() != 1 { return Err(unsupported()); }
    let root = PathBuf::from(roots[0]);
    if !verified_roots.contains(&root) || fs::canonicalize(&root).map_err(|_| unsupported())? != root
        || [".env", ".op.env"].iter().any(|name| root.join(name).symlink_metadata().is_ok()) { return Err(unsupported()); }
    // Validate ALL fixed public paths before the interpreter can import installed source.
    let source = public_source_fingerprint(&root, fixture)?;
    if !candidates.iter().any(|build| build["versionLine"].as_str() == Some(version_line)
        && build["sourceTreeSha256"].as_str() == Some(&source)) { return Err(unsupported()); }
    let probe_path = scratch.0.join("probe.py");
    let fixture_path = scratch.0.join("manifest.json");
    fs::write(&probe_path, SAFE_PROBE).map_err(|_| unsupported())?;
    fs::write(&fixture_path, COMPATIBILITY_FIXTURE).map_err(|_| unsupported())?;
    let mut diagnostic = command(&python, &["-I", "-B"]);
    diagnostic.arg(&probe_path).arg(&root).arg(&fixture_path);
    let bytes = run(diagnostic)?;
    let result: Value = serde_json::from_slice(&bytes).map_err(|_| unsupported())?;
    if result.as_object().is_none_or(|fields| fields.len() != 5)
        || result["passed"] != true || result["sdkJsonSseToolParsing"] != true
        || result["sourceTreeSha256"].as_str() != Some(&source) { return Err(unsupported()); }
    if !candidates.iter().any(|build| evidence_matches(build, version_line, &entrypoint, &source,
        result["pythonVersion"].as_str().unwrap_or(""), result["anthropicVersion"].as_str().unwrap_or(""))) { return Err(unsupported()); }
    // Help only: no config values, chat, helper, gateway or write for discovery.
    for method in ["get", "set", "unset"] {
        let help = run(command(&target.executable, &["config", method, "--help"]))?;
        let help = std::str::from_utf8(&help).map_err(|_| unsupported())?;
        if !help.contains(&format!("hermes config {method}")) || !help.contains("usage:") { return Err(unsupported()); }
    }
    // Detect source/launcher drift during the diagnostic too, not merely at discovery.
    if fs::read(&target.executable).map_err(|_| unsupported())?.as_slice() != script.as_bytes()
        || public_source_fingerprint(&root, fixture)? != source { return Err(unsupported()); }
    Ok(())
}

fn public_source_fingerprint(root: &Path, fixture: &Value) -> HermesResult<String> {
    let files = fixture["publicSourceFiles"].as_array().ok_or(HermesError::Unsupported)?;
    if files.is_empty() { return Err(HermesError::Unsupported); }
    let mut names = files.iter().map(|name| name.as_str().ok_or(HermesError::Unsupported)).collect::<HermesResult<Vec<_>>>()?;
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) { return Err(HermesError::Unsupported); }
    let mut digest = Sha256::new();
    for name in names {
        let relative = Path::new(name);
        if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) { return Err(HermesError::Unsupported); }
        let path = root.join(relative);
        // No symlinks into credential files, including symlinked parent directories.
        if fs::canonicalize(&path).map_err(|_| HermesError::Unsupported)? != path { return Err(HermesError::Unsupported); }
        let bytes = fs::read(&path).map_err(|_| HermesError::Unsupported)?;
        digest.update(name.as_bytes()); digest.update([0]); digest.update(Sha256::digest(bytes));
    }
    Ok(format!("{:x}", digest.finalize()))
}

struct ProbeHome(PathBuf);
impl ProbeHome {
    fn new() -> HermesResult<Self> {
        let path = std::env::temp_dir().join(format!("keyzapper-hermes-probe-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)] { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
        builder.create(&path).map_err(|_| HermesError::Unsupported)?;
        let home = Self(path);
        for name in ["user", "hermes"] { fs::create_dir(home.0.join(name)).map_err(|_| HermesError::Unsupported)?; }
        Ok(home)
    }
}
impl Drop for ProbeHome { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn run_command(mut command: Command, mut try_wait: impl FnMut(&mut std::process::Child) -> std::io::Result<Option<std::process::ExitStatus>>) -> HermesResult<Vec<u8>> {
    #[cfg(not(unix))]
    { let _ = (&mut command, &mut try_wait); return Err(HermesError::Unsupported); }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)] command.process_group(0);
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => HermesError::MissingInstallation,
        _ => HermesError::CliFailed,
    })?;
    let mut out = child.stdout.take().ok_or(HermesError::CliFailed)?;
    let mut err = child.stderr.take().ok_or(HermesError::CliFailed)?;
    let stdout = thread::spawn(move || drain_bounded(&mut out));
    let stderr = thread::spawn(move || drain_bounded(&mut err));
    let start = Instant::now();
    let status = loop {
        match try_wait(&mut child) {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= TIMEOUT => { terminate(&mut child); let _ = stdout.join(); let _ = stderr.join(); return Err(HermesError::Timeout); }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(_) => { terminate(&mut child); let _ = stdout.join(); let _ = stderr.join(); return Err(HermesError::CliFailed); }
        }
    };
    #[cfg(unix)] kill_group(child.id());
    let out_result = stdout.join(); let err_result = stderr.join();
    let bytes = out_result.map_err(|_| HermesError::CliFailed)?.map_err(|_| HermesError::CliFailed)?;
    err_result.map_err(|_| HermesError::CliFailed)?.map_err(|_| HermesError::CliFailed)?;
    if !status.success() { return Err(HermesError::CliFailed); }
    Ok(bytes)
}

impl ConfigAccess for HermesCli {
    fn capability(&self, target: &HermesTarget) -> Capability { HermesCli::capability(self, target) }
    fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> {
        self.check_write(target)?;
        let path = self.path(target)?;
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(HermesError::InvalidConfig),
        }
    }
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
        self.check_write(target)?;
        let path = self.path(target)?;
        let data = match fs::read(&path) { Ok(bytes) => bytes, Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(), Err(_) => return Err(HermesError::InvalidConfig) };
        let parsed: Value = if data.is_empty() { Value::Object(Default::default()) } else { serde_yaml_ng::from_slice(&data).map_err(|_| HermesError::InvalidConfig)? };
        if !parsed.is_object() { return Err(HermesError::InvalidConfig); }
        let fields = keys.iter().map(|key| {
            let value = key.split('.').try_fold(&parsed, |at, part| at.get(part));
            (key.clone(), value.cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent))
        }).collect();
        let digest = format!("{:x}", Sha256::digest(&data));
        Ok(ConfigSnapshot { config_path: path, config_digest: digest, fields })
    }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> {
        self.check_write(target)?;
        self.path(target)?;
        if !allowed_value(key, value) { return Err(HermesError::PolicyDenied); }
        // Native `config set` preserves string-typed argv literally (including any quotes).
        let serialized = match value.as_str() {
            Some(text) => text.to_owned(),
            None => serde_json::to_string(value).map_err(|_| HermesError::InvalidConfig)?,
        };
        self.run_write(target, vec!["config".into(), "set".into(), key.into(), serialized])
    }
    fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> {
        self.check_write(target)?;
        self.path(target)?;
        if !allowed_key(key) { return Err(HermesError::PolicyDenied); }
        self.run_write(target, vec!["config".into(), "unset".into(), key.into()])
    }
}
fn allowed_key(key: &str) -> bool {
    matches!(key, "providers.keyzapper" | "model.provider" | "model.default" | "model.base_url")
}

/// Build the only accepted KeyZapper provider payload for Task 4 and write validation.
pub fn desired_provider(base_url: &str, helper: &Path, profile_id: &str) -> Option<Value> {
    if !valid_base_url(base_url) || !valid_helper_path(helper) || !valid_profile_id(profile_id) { return None; }
    let command = format!("{} credential --profile {} --json", shell_quote(helper.to_str()?), profile_id);
    Some(serde_json::json!({ "base_url": base_url, "api_mode": "anthropic_messages", "key_cmd": command }))
}

fn valid_base_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else { return false; };
    if value.trim() != value || value.chars().any(char::is_control)
        || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() { return false; }
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => {
            let loopback = match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                None => false,
            };
            loopback
        }
        _ => false,
    }
}

fn valid_profile_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string().to_uppercase() == value)
}
fn valid_helper_path(path: &Path) -> bool {
    path.is_absolute() && path.file_name().is_some_and(|name| name == "keyzapper-helper")
        && path.components().all(|part| !matches!(part, std::path::Component::ParentDir | std::path::Component::CurDir))
}
fn shell_quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }
fn parse_quoted_path(value: &str) -> Option<(String, &str)> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&39) { return None; }
    let mut raw = String::new();
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == 39 {
            if bytes.get(i..i + 4) == Some(&[39, 92, 39, 39]) {
                raw.push('\''); i += 4; continue;
            }
            let rest = value.get(i + 1..)?;
            if shell_quote(&raw) != value.get(..i + 1)? { return None; }
            return Some((raw, rest));
        }
        let ch = value.get(i..)?.chars().next()?;
        raw.push(ch); i += ch.len_utf8();
    }
    None
}
fn valid_key_command(value: &str) -> bool {
    let Some((path, rest)) = parse_quoted_path(value) else { return false; };
    if !rest.starts_with(" credential --profile ") { return false; }
    let Some((profile, trailing)) = rest[" credential --profile ".len()..].split_once(" --json") else { return false; };
    if trailing != "" || !valid_profile_id(profile) { return false; }
    valid_helper_path(Path::new(&path))
}

/// Validate the exact provider schema and the safe, supported values for each field.
pub fn validate_provider_config(value: &Value) -> bool {
    let Some(fields) = value.as_object() else { return false; };
    fields.len() == 3
        && fields.get("api_mode").and_then(Value::as_str) == Some("anthropic_messages")
        && fields.get("base_url").and_then(Value::as_str).is_some_and(valid_base_url)
        && fields.get("key_cmd").and_then(Value::as_str).is_some_and(valid_key_command)
}

fn safe_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|c| c.is_ascii_alphanumeric() || b"._:/-".contains(&c)) { return false; }
    let lower = value.to_ascii_lowercase();
    !["sk-", "pk-", "token", "secret", "password", "bearer", "credential"].iter().any(|marker| lower.contains(marker))
}
pub fn safe_model_identifier(value: &str) -> bool { safe_identifier(value) }
pub fn safe_existing_value(key: &str, value: &Value) -> bool {
    match key {
        "providers.keyzapper" => validate_provider_config(value),
        "model.provider" | "model.default" => value.as_str().is_some_and(safe_identifier),
        "model.base_url" => value.as_str().is_some_and(valid_base_url),
        _ => false,
    }
}
fn allowed_value(key: &str, value: &Value) -> bool {
    allowed_key(key) && safe_existing_value(key, value)
}

#[cfg(unix)]
fn kill_group(id: u32) { unsafe { libc::kill(-(id as i32), libc::SIGKILL); } }
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)] kill_group(child.id());
    let _ = child.kill(); let _ = child.wait();
}
fn drain_bounded(reader: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut saved = Vec::new(); let mut buf = [0u8; 8192];
    loop { let n = reader.read(&mut buf)?; if n == 0 { break; } let keep = OUTPUT_LIMIT.saturating_sub(saved.len()).min(n); saved.extend_from_slice(&buf[..keep]); }
    Ok(saved)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery { pub executables: Vec<PathBuf>, pub homes: Vec<PathBuf>, pub supported: bool, pub diagnostics: Vec<InstallationDiagnostic> }
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompatibilityReason { UnknownBuild, NotValidated }
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationDiagnostic { pub executable: PathBuf, pub reason: CompatibilityReason }
impl InstallationDiagnostic {
    /// Public launcher bytes only. Never execute a CLI or resolve a user's home.
    pub fn for_executable(executable: &Path) -> Self {
        let known = fs::read(executable).ok().filter(|bytes| bytes.len() <= 4096).is_some_and(|bytes| {
            let hash = format!("{:x}", Sha256::digest(bytes));
            serde_json::from_str::<Value>(COMPATIBILITY_FIXTURE).ok().is_some_and(|fixture|
                fixture["supportedBuilds"].as_array().is_some_and(|builds| builds.iter().any(|build|
                    evidence_is_complete(build) && build["entrypointSha256"].as_str() == Some(&hash))))
        });
        Self { executable: executable.into(), reason: if known { CompatibilityReason::NotValidated } else { CompatibilityReason::UnknownBuild } }
    }
}
pub fn default_home(executable: &Path) -> Option<PathBuf> {
    if !executable.is_absolute() { return None; }
    let target = HermesTarget { executable: executable.into(), home: std::env::temp_dir(), display_name: String::new() };
    HermesCli::new().verify(&target).ok()?;
    resolve_default_home(executable)
}

fn resolve_default_home(executable: &Path) -> Option<PathBuf> {
    let scratch = ProbeHome::new().ok()?;
    let mut command = Command::new(executable);
    command.args(["config", "path"]).env_clear().current_dir(&scratch.0)
        .env("PATH", "/usr/bin:/bin").env("TMPDIR", &scratch.0)
        .env("PYTHONDONTWRITEBYTECODE", "1").env("PYTHONPYCACHEPREFIX", scratch.0.join("bytecode"))
        .env("LANG", "C.UTF-8").env("NO_COLOR", "1");
    if let Some(home) = std::env::var_os("HOME") { command.env("HOME", home); }
    let output = run_command(command, |child| child.try_wait()).ok()?;
    if output.len() > 64 * 1024 { return None; }
    let path = PathBuf::from(std::str::from_utf8(&output).ok()?.trim());
    let home = path.parent()?;
    fs::canonicalize(home).ok()
}

pub fn discover(root: Option<&Path>) -> Discovery {
    if !cfg!(target_os = "macos") { return Discovery { executables: vec![], homes: vec![], supported: false, diagnostics: vec![] }; }
    let mut executables = vec![PathBuf::from("/opt/homebrew/bin/hermes"), PathBuf::from("/usr/local/bin/hermes")];
    if let Some(path) = std::env::var_os("PATH") { for dir in std::env::split_paths(&path) { executables.push(dir.join("hermes")); } }
    executables.retain(|p| p.is_file());
    executables = executables.into_iter().filter_map(|p| fs::canonicalize(p).ok()).collect();
    executables.sort(); executables.dedup();
    discover_installations(executables, root)
}
fn discover_installations(executables: Vec<PathBuf>, root: Option<&Path>) -> Discovery {
    discover_installations_with(executables, root, default_home)
}
/// A resolved home is one target: itself plus its named `profiles/*` children.
fn homes_under(root: &Path) -> Vec<PathBuf> {
    let Ok(root) = fs::canonicalize(root) else { return vec![] };
    let mut homes = vec![root.clone()];
    if let Ok(entries) = fs::read_dir(root.join("profiles")) {
        homes.extend(entries.flatten().filter(|e| e.path().is_dir()).filter_map(|e| fs::canonicalize(e.path()).ok()));
    }
    homes
}
/// Private seam: production resolves defaults through the verified CLI, tests inject one.
fn discover_installations_with(executables: Vec<PathBuf>, root: Option<&Path>, resolve_default: impl Fn(&Path) -> Option<PathBuf>) -> Discovery {
    let mut homes = Vec::new();
    match root {
        // Explicit roots are used exactly as given, named profiles included.
        Some(root) => homes.extend(homes_under(root)),
        // A resolved default root has named profiles too; do not hardcode ~/.hermes here.
        None => for executable in &executables {
            if let Some(root) = resolve_default(executable) { homes.extend(homes_under(&root)); }
        },
    }
    homes.sort(); homes.dedup();
    let diagnostics = executables.iter().map(|exe| InstallationDiagnostic::for_executable(exe)).collect();
    Discovery { executables, homes, supported: true, diagnostics }
}

// The production verifier only supports Unix installations (console-script entrypoints and
// POSIX shell fixtures below), so its synthetic transport is Unix-only too. Windows still
// compiles the production modules and the portable registry/lifecycle tests.
#[cfg(all(test, unix))]
#[path = "cli_verifier_tests.rs"]
mod verifier_tests;
#[cfg(all(test, unix))]
pub(crate) use verifier_tests::RecordedVerifier;

#[cfg(all(test, unix))]
#[path = "cli_read_tests.rs"]
mod read_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn unknown_only_inventory_without_home_keeps_nonexecuting_installation_diagnostic() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = ProbeHome::new().unwrap();
        let executable = scratch.0.join("unknown-hermes");
        let marker = scratch.0.join("must-not-execute");
        fs::write(&executable, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let found = discover_installations(vec![executable.clone()], None);
        assert!(found.homes.is_empty());
        assert_eq!(found.executables, vec![executable.clone()]);
        let diagnostics = serde_json::to_value(&found.diagnostics).unwrap();
        assert_eq!(diagnostics, serde_json::json!([{ "executable": executable, "reason": "unknownBuild" }]));
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn direct_snapshot_and_raw_config_deny_unknown_launcher_before_config_path() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = ProbeHome::new().unwrap();
        let marker = scratch.0.join("executed");
        let target = HermesTarget { executable: scratch.0.join("unknown"), home: scratch.0.join("hermes"), display_name: "unknown".into() };
        fs::write(&target.executable, format!("#!/bin/sh\ntouch '{}'\nprintf '%s\\n' \"$HERMES_HOME/config.yaml\"\n", marker.display())).unwrap();
        fs::set_permissions(&target.executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(target.home.join("config.yaml"), "private: synthetic-secret\n").unwrap();
        let cli = HermesCli::new();
        assert_eq!(cli.snapshot(&target, &[]).err(), Some(HermesError::Unsupported));
        assert_eq!(cli.raw_config(&target), Err(HermesError::Unsupported));
        assert!(!marker.exists());
    }

    #[test]
    fn default_root_discovery_includes_named_profiles_without_executing_unknown_launchers() {
        let scratch = ProbeHome::new().unwrap();
        let root = scratch.0.join("default-home");
        fs::create_dir_all(root.join("profiles/work")).unwrap();
        let executable = scratch.0.join("unknown-hermes");
        let marker = scratch.0.join("must-not-execute");
        fs::write(&executable, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        // No HERMES_HOME: the default root still contributes its named profiles.
        let found = discover_installations_with(vec![executable.clone()], None, |_| Some(root.clone()));
        let mut expected = vec![fs::canonicalize(&root).unwrap(), fs::canonicalize(root.join("profiles/work")).unwrap()];
        expected.sort();
        assert_eq!(found.homes, expected);
        assert_eq!(found.executables, vec![executable]);
        assert_eq!(found.diagnostics.len(), 1);
        assert!(!marker.exists(), "unknown launcher must not be executed during discovery");
    }

    #[cfg(unix)]
    #[test]
    fn default_discovery_never_executes_an_unknown_installation_with_user_home_access() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = ProbeHome::new().unwrap();
        let executable = scratch.0.join("hermes-cli");
        let marker = scratch.0.join("executed");
        fs::write(&executable, format!("#!/bin/sh\ntouch '{}'\nprintf '%s\\n' '{}'\n", marker.display(), scratch.0.join("hermes/config.yaml").display())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(default_home(&executable), None);
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn default_home_uses_selected_cli_config_path_without_forcing_home() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("hermes-default-home-{}", uuid::Uuid::new_v4()));
        let home = root.join("canonical-default");
        fs::create_dir_all(&home).unwrap();
        let executable = root.join("fake-hermes");
        fs::write(&executable, format!("#!/bin/sh\n[ -z \"$HERMES_HOME\" ] || exit 9\nprintf '%s\\n' '{}'\n", home.join("config.yaml").display())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(resolve_default_home(&executable), Some(fs::canonicalize(&home).unwrap()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_schema_rejects_secret_payloads_and_unowned_keys() {
        assert!(!allowed_value("model.default", &serde_json::json!("secret-token-value")));
        assert!(!allowed_value("providers.keyzapper", &serde_json::json!({"api_key": "secret"})));
        assert!(!allowed_value("model.api_key", &serde_json::json!("x")));
        assert!(allowed_value("model.provider", &serde_json::json!("keyzapper")));
        let root = std::env::temp_dir().join(format!("hermes-provider-schema-{}", uuid::Uuid::new_v4()));
        let helper_dir = root.join("Key Zapper's.app/Contents/Helpers");
        fs::create_dir_all(&helper_dir).unwrap();
        let helper = helper_dir.join("keyzapper-helper");
        fs::write(&helper, b"test fixture").unwrap();
        let profile = "550E8400-E29B-41D4-A716-446655440000";
        let provider = desired_provider("https://gateway.example/v1", &helper, profile).unwrap();
        let quoted_helper = shell_quote(helper.to_str().unwrap());
        assert_eq!(provider["key_cmd"], format!("{quoted_helper} credential --profile {profile} --json"));
        assert_eq!(provider["base_url"], "https://gateway.example/v1");
        assert_eq!(provider["api_mode"], "anthropic_messages");
        assert!(allowed_value("providers.keyzapper", &provider));
        fs::remove_dir_all(root).unwrap();
        for invalid in [
            serde_json::json!({"name":"KeyZapper"}),
            serde_json::json!({"base_url":"https://gateway.example/v1","api_mode":"anthropic_messages","key_cmd":provider["key_cmd"],"name":"secret-token-value"}),
            serde_json::json!({"base_url":"https://gateway.example/v1","api_mode":"anthropic_messages","key_cmd":"echo secret-token-value"}),
        ] { assert!(!allowed_value("providers.keyzapper", &invalid)); }
        assert!(!allowed_value("providers.keyzapper.extra", &serde_json::json!("value")));
        assert!(!allowed_value("model.default", &serde_json::json!("sk-abc123")));
        assert!(!allowed_value("model.provider", &serde_json::json!("provider-token-value")));
        assert!(allowed_value("model.default", &serde_json::json!("claude-3-7-sonnet")));
        assert!(allowed_value("model.base_url", &serde_json::json!("http://127.0.0.1:8080/v1")));
        assert!(!allowed_value("model.base_url", &serde_json::json!("https://user:pass@gateway.example/v1")));
    }
    #[test]
    fn evidence_is_bound_to_the_selected_installation_and_support_status() {
        let target = HermesTarget { executable: "hermes".into(), home: "/home/a".into(), display_name: "a".into() };
        let supported = HermesCli::with_evidence(CompatibilityEvidence::validated(&target, true));
        assert!(supported.check_write(&target).is_ok());
        let mut other = target.clone(); other.home = "/home/b".into();
        assert_eq!(supported.check_write(&other), Err(HermesError::Unsupported));
        let unsupported = HermesCli::with_evidence(CompatibilityEvidence::validated(&target, false));
        assert_eq!(unsupported.check_write(&target), Err(HermesError::Unsupported));
        assert_eq!(HermesCli::new().check_write(&target), Err(HermesError::Unsupported));
    }

    #[test]
    fn compatibility_requires_exact_complete_embedded_evidence() {
        let fixture: Value = serde_json::from_str(COMPATIBILITY_FIXTURE).unwrap();
        let build = &fixture["supportedBuilds"][0];
        assert!(evidence_matches(build, "Hermes Agent vunknown (2026.9.24)", "ba3423a40ad932ba1139c55fd5a4a538ac8cfdcd04ac5586807ca567a601db16", "eea973ec5b6d5be59b87d4a453a31a2a7c06d7bd63891fd16cf97927de2f017c", "3.14.10", "0.125.0"));
        assert!(!evidence_matches(build, "Hermes Agent vunknown", "ba3423a40ad932ba1139c55fd5a4a538ac8cfdcd04ac5586807ca567a601db16", "eea973ec5b6d5be59b87d4a453a31a2a7c06d7bd63891fd16cf97927de2f017c", "3.14.10", "0.125.0"));
        assert!(!evidence_matches(build, "Hermes Agent vunknown (2026.9.24)", "wrong", "eea973ec5b6d5be59b87d4a453a31a2a7c06d7bd63891fd16cf97927de2f017c", "3.14.10", "0.125.0"));
        assert!(!evidence_matches(build, "Hermes Agent vunknown (2026.9.24)", "ba3423a40ad932ba1139c55fd5a4a538ac8cfdcd04ac5586807ca567a601db16", "wrong", "3.14.10", "0.125.0"));
        assert!(!evidence_matches(build, "Hermes Agent vunknown (2026.9.24)", "ba3423a40ad932ba1139c55fd5a4a538ac8cfdcd04ac5586807ca567a601db16", "eea973ec5b6d5be59b87d4a453a31a2a7c06d7bd63891fd16cf97927de2f017c", "3.14.9", "0.125.0"));
        let mut partial = build.clone(); partial["fullPass"] = Value::Bool(false);
        assert!(!evidence_is_complete(&partial));
        let mut partial = build.clone(); partial["checks"]["selected_launcher_chat"] = Value::Bool(false);
        assert!(!evidence_is_complete(&partial));
        let mut renamed = build.clone();
        renamed["checks"].as_object_mut().unwrap().remove("selected_launcher_chat");
        renamed["checks"]["invented_check"] = Value::Bool(true);
        assert!(!evidence_is_complete(&renamed), "thirteen arbitrary true checks are not acceptance");
        let mut native = build.clone(); native["nativeResult"]["checks"]["selected_launcher_chat"] = Value::Bool(false);
        assert!(!evidence_is_complete(&native));
        assert!(!evidence_matches(build, "Hermes Agent vunknown (2026.9.24)", "ba3423a40ad932ba1139c55fd5a4a538ac8cfdcd04ac5586807ca567a601db16", "eea973ec5b6d5be59b87d4a453a31a2a7c06d7bd63891fd16cf97927de2f017c", "3.14.10", "wrong"));
    }

    #[cfg(unix)]
    #[test]
    fn injected_try_wait_error_kills_and_reaps_the_child() {
        let cli = HermesCli::new();
        let target = HermesTarget { executable: "/bin/sh".into(), home: std::env::temp_dir(), display_name: "test".into() };
        let mut child_id = None;
        let result = cli.execute_with_wait(&target, &["-c".into(), "exec sleep 30".into()], |child| {
            child_id = Some(child.id());
            Err(std::io::Error::other("injected try_wait failure"))
        });
        assert_eq!(result, Err(HermesError::CliFailed));
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child_id.unwrap() as i32, &mut status, libc::WNOHANG) };
        assert_eq!(waited, -1, "child was not reaped");
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
    }

    #[cfg(unix)]
    #[test]
    fn evidence_enabled_writes_validate_path_and_execute_set_and_unset() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("hermes-cli-unit-{}", uuid::Uuid::new_v4()));
        let home = root.join("selected home");
        fs::create_dir_all(&home).unwrap();
        let executable = root.join("fake-hermes");
        fs::write(&executable, "#!/bin/sh\nif [ \"$1 $2\" = 'config path' ]; then printf '%s\\n' \"$HERMES_HOME/config.yaml\"; else printf '%s\\n' \"$*\" >> \"$HERMES_HOME/writes\"; fi\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let target = HermesTarget { executable, home: home.clone(), display_name: "test".into() };
        let cli = HermesCli::with_evidence(CompatibilityEvidence::validated(&target, true));
        cli.set(&target, "model.default", &serde_json::json!("model-x")).unwrap();
        cli.unset(&target, "model.default").unwrap();
        // The installed CLI preserves string values verbatim; JSON quotes become part of the model ID.
        assert_eq!(fs::read_to_string(home.join("writes")).unwrap(), "config set model.default model-x\nconfig unset model.default\n");

        fs::write(&target.executable, "#!/bin/sh\nif [ \"$1 $2\" = 'config path' ]; then printf '%s\\n' /etc/hermes/config.yaml; else touch \"$HERMES_HOME/escaped-write\"; fi\n").unwrap();
        assert_eq!(cli.set(&target, "model.default", &serde_json::json!("model-y")), Err(HermesError::WrongTarget));
        assert!(!home.join("escaped-write").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
