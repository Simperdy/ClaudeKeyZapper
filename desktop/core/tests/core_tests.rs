use keyzapper_core::backup::{self, BackupError, BackupPayload};
use keyzapper_core::claude_office::OfficeClient;
use keyzapper_core::errors::Error;
use keyzapper_core::helper::{ExitCode, HelperCommand};
use keyzapper_core::keys::KeyStore;
use keyzapper_core::managed::ManagedConfig;
use keyzapper_core::metadata::MetadataStore;
use keyzapper_core::models::{AppState, AuthType, ModelSelection, Profile, WorkspaceBinding};
use keyzapper_core::tokens::{TokenSet, TokenStore};
use keyzapper_core::paths::canonical_path;
use keyzapper_core::settings::{self, BindingHealth, SettingsBinder, GIT_EXCLUDE_LINE};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const ALPHA: &str = "6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9";
const BETA: &str = "0A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9";

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kz-{}", uuid::Uuid::new_v4())).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    canonical_path(&dir.to_string_lossy())
}

fn sample() -> Profile {
    let mut p = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test/".into(), "claude-alpha".into());
    p.opus_model = Some("team-opus".into());
    p
}

fn other() -> Profile {
    Profile::new(BETA.into(), "Beta".into(), "https://litellm-b.example.test".into(), String::new())
}

fn binder() -> SettingsBinder {
    SettingsBinder {
        helper_path: "/Applications/Project AI'Switch.app/Contents/MacOS/keyzapper-helper".into(),
        managed_settings_paths: vec![],
        user_settings_path: "/nonexistent/settings.json".into(),
    }
}

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn write(path: &str, value: Value) {
    std::fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).current_dir(dir).output().unwrap();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_init(dir: &str) {
    assert!(git(dir, &["init", "-q"]).is_some() && git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]).is_some());
}

fn local(root: &str) -> String {
    format!("{root}/.claude/settings.local.json")
}

#[test]
fn applies_idempotently_and_preserves_other_settings() {
    let root = temp_dir("proj with space");
    write(&local(&root), json!({"permissions": {"allow": ["Bash(ls)"]}, "env": {"FOO": "bar"}}));
    let first = binder().apply(&sample(), &root, None, false, None).unwrap();
    assert!(first.changed);
    let settings = read(&local(&root));
    assert_eq!(settings["permissions"]["allow"], json!(["Bash(ls)"]));
    assert_eq!(settings["env"]["FOO"], "bar");
    assert_eq!(settings["env"]["ANTHROPIC_BASE_URL"], "https://litellm.example.test");
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "claude-alpha");
    assert_eq!(settings["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "team-opus");
    assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "");
    assert!(settings["apiKeyHelper"].as_str().unwrap().ends_with(&format!("credential --profile {ALPHA}")));

    let bytes = std::fs::read(local(&root)).unwrap();
    let second = binder().apply(&sample(), &root, Some(&first.binding), false, None).unwrap();
    assert!(!second.changed);
    assert_eq!(second.binding, first.binding);
    assert_eq!(std::fs::read(local(&root)).unwrap(), bytes);
    assert_eq!(binder().inspect(&first.binding, &sample(), &BTreeMap::new()).health, BindingHealth::Active);
}

#[cfg(unix)]
#[test]
fn helper_command_survives_spaces_and_quotes_in_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("helper dir's");
    let helper = format!("{dir}/keyzapper-helper");
    std::fs::write(&helper, "#!/bin/sh\nprintf 'ok:%s' \"$3\"\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let b = SettingsBinder { helper_path: helper, ..binder() };
    let command = b.desired_values(&sample(), false, None)["apiKeyHelper"].clone();
    let out = Command::new("/bin/sh").arg("-c").arg(&command).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("ok:{ALPHA}"));
}

/// A project inside WSL runs Claude Code as a Linux process: the Windows helper is called through WSL interop under
/// `/bin/sh`, by its `/mnt/<drive>/…` path, quoted for sh. The status check compares against the same command.
#[test]
fn wsl_projects_get_the_helper_as_sh_command_on_its_mnt_path() {
    let b = SettingsBinder { helper_path: r"C:\Users\u\AppData\Local\Programs\Key Zapper\keyzapper-helper.exe".into(), ..binder() };
    let root = r"\\wsl.localhost\Ubuntu\home\u\my proj";
    let command = b.desired_values_for(root, &sample(), false, None)["apiKeyHelper"].clone();
    assert_eq!(command, format!("'/mnt/c/Users/u/AppData/Local/Programs/Key Zapper/keyzapper-helper.exe' credential --profile {ALPHA}"));
    assert_eq!(b.desired_values_for(r"\\wsl$\Ubuntu\home\u", &sample(), true, None)["apiKeyHelper"], command.replace("credential", "pool"));
    assert_eq!(settings::helper_invocation(r"C:\x\it's.exe", r"\\wsl$\Ubuntu"), "'/mnt/c/x/it'\\''s.exe'");
    // Folders outside WSL keep the platform's quoting; the user settings (global default) are never inside WSL.
    assert_eq!(b.desired_values_for(r"C:\proj", &sample(), false, None)["apiKeyHelper"], b.desired_values(&sample(), false, None)["apiKeyHelper"]);
    assert_eq!(settings::helper_invocation(r"C:\x\keyzapper-helper.exe", r"C:\proj"), settings::shell_quote(r"C:\x\keyzapper-helper.exe"));
}

/// A helper that WSL cannot reach through `/mnt/<drive>` (e.g. installed on a network share) blocks a WSL project
/// instead of being written as a command `/bin/sh` cannot run; folders outside WSL are unaffected.
#[test]
fn wsl_projects_refuse_a_helper_outside_a_drive_letter() {
    let conflict = settings::wsl_helper_conflict(r"\\srv\apps\KeyZapper\keyzapper-helper.exe", r"\\wsl.localhost\Ubuntu\home\u\proj").unwrap();
    assert!(conflict.blocking && conflict.message.contains("/mnt/"));
    assert!(settings::wsl_helper_conflict(r"C:\Programs\KeyZapper\keyzapper-helper.exe", r"\\wsl.localhost\Ubuntu\home\u\proj").is_none());
    assert!(settings::wsl_helper_conflict(r"\\srv\apps\KeyZapper\keyzapper-helper.exe", r"C:\proj").is_none());
    let b = SettingsBinder { helper_path: r"\\srv\apps\KeyZapper\keyzapper-helper.exe".into(), ..binder() };
    assert!(b.apply(&sample(), &temp_dir("proj"), None, false, None).is_ok());
}

/// Claude Code runs `apiKeyHelper` through `cmd /d /s /c` on Windows.
#[cfg(windows)]
#[test]
fn helper_command_survives_spaces_in_path_under_cmd() {
    use std::os::windows::process::CommandExt;
    let dir = temp_dir("helper dir");
    let helper = format!("{dir}\\keyzapper-helper.cmd");
    std::fs::write(&helper, "@echo ok:%3\r\n").unwrap();
    let b = SettingsBinder { helper_path: helper, ..binder() };
    let command = b.desired_values(&sample(), false, None)["apiKeyHelper"].clone();
    let out = Command::new("cmd.exe").raw_arg(format!("/d /s /c \"{command}\"")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("ok:{ALPHA}"));
}

#[test]
fn toggles_pool_mode_and_removes_its_cache_setting_again() {
    let root = temp_dir("proj");
    let pooled = binder().apply(&sample(), &root, None, true, None).unwrap();
    assert!(pooled.binding.is_pooled());
    assert!(read(&local(&root))["apiKeyHelper"].as_str().unwrap().ends_with(&format!("pool --profile {ALPHA}")));
    let plain = binder().apply(&sample(), &root, Some(&pooled.binding), false, None).unwrap();
    assert_eq!(plain.binding.pooled, None);
    assert!(read(&local(&root))["env"].get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").is_none());
    assert_eq!(binder().inspect(&plain.binding, &sample(), &BTreeMap::new()).health, BindingHealth::Active);
}

#[test]
fn foreign_values_are_conflicts_and_left_untouched() {
    let root = temp_dir("proj");
    write(&local(&root), json!({"apiKeyHelper": "/usr/local/bin/other", "env": {"ANTHROPIC_API_KEY": "sk-user"}}));
    let before = std::fs::read(local(&root)).unwrap();
    match binder().apply(&sample(), &root, None, false, None) {
        Err(Error::SettingsConflicts(conflicts)) => {
            assert_eq!(conflicts.len(), 2);
            assert!(!conflicts.iter().any(|c| c.message.contains("sk-user")));
        }
        other => panic!("expected conflicts, got {:?}", other.err()),
    }
    assert_eq!(std::fs::read(local(&root)).unwrap(), before);
}

#[test]
fn switching_profile_replaces_only_own_values() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false, None).unwrap();
    let b = binder().apply(&other(), &root, Some(&a.binding), false, None).unwrap();
    let env = &read(&local(&root))["env"];
    assert_eq!(env["ANTHROPIC_BASE_URL"], "https://litellm-b.example.test");
    assert!(env.get("ANTHROPIC_MODEL").is_none());
    assert_eq!(b.binding.id, a.binding.id);
    assert_eq!(b.binding.profile_id, BETA);
}

#[test]
fn user_edited_value_blocks_reapply_and_survives_revert() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false, None).unwrap();
    let mut settings = read(&local(&root));
    settings["env"]["ANTHROPIC_MODEL"] = json!("user-choice");
    settings["model"] = json!("opus");
    write(&local(&root), settings);
    assert_eq!(
        binder().inspect(&a.binding, &sample(), &BTreeMap::new()).health,
        BindingHealth::Drifted(vec!["env.ANTHROPIC_MODEL".into()])
    );
    assert!(binder().apply(&sample(), &root, None, false, None).is_err());
    assert_eq!(binder().revert(&a.binding).unwrap(), vec!["env.ANTHROPIC_MODEL".to_string()]);
    let after = read(&local(&root));
    assert!(after.get("apiKeyHelper").is_none());
    assert_eq!(after["model"], "opus");
    assert_eq!(after["env"], json!({"ANTHROPIC_MODEL": "user-choice"}));
}

#[test]
fn revert_deletes_file_created_only_for_the_binding() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false, None).unwrap();
    assert!(binder().revert(&a.binding).unwrap().is_empty());
    assert!(!Path::new(&local(&root)).exists());
    assert!(!Path::new(&format!("{root}/.claude")).exists());
    assert_eq!(binder().inspect(&a.binding, &sample(), &BTreeMap::new()).health, BindingHealth::NotApplied);
}

#[test]
fn invalid_json_is_not_overwritten_and_missing_folder_is_reported() {
    let root = temp_dir("proj");
    std::fs::create_dir_all(format!("{root}/.claude")).unwrap();
    std::fs::write(local(&root), "{ broken").unwrap();
    assert!(binder().apply(&sample(), &root, None, false, None).is_err());
    assert_eq!(std::fs::read_to_string(local(&root)).unwrap(), "{ broken");

    let gone = format!("{root}/gone");
    let binding = WorkspaceBinding::new(gone.clone(), ALPHA.into());
    assert_eq!(binder().inspect(&binding, &sample(), &BTreeMap::new()).health, BindingHealth::FolderMissing);
    assert_eq!(binder().apply(&sample(), &gone, None, false, None).err(), Some(Error::FolderNotFound(canonical_path(&gone))));
}

#[test]
fn trailing_comma_is_reported_and_repaired_by_reapply() {
    let root = temp_dir("proj");
    let applied = binder().apply(&sample(), &root, None, false, None).unwrap();
    let text = std::fs::read_to_string(local(&root)).unwrap();
    std::fs::write(local(&root), text.replacen("\"apiKeyHelper\"", "\"model\": \"opus\",\n  \"apiKeyHelper\"", 1).replace("\n}", ",\n}")).unwrap();
    assert!(settings::is_unparsable_json(&local(&root)));
    let status = binder().inspect(&applied.binding, &sample(), &BTreeMap::new());
    assert!(status.conflicts.iter().any(|c| c.blocking));
    let again = binder().apply(&sample(), &root, Some(&applied.binding), false, None).unwrap();
    assert!(again.changed);
    assert!(!settings::is_unparsable_json(&local(&root)));
    assert_eq!(read(&local(&root))["model"], "opus");
}

#[test]
fn managed_settings_and_provider_switches_block() {
    let root = temp_dir("proj");
    let managed = format!("{}/managed-settings.json", temp_dir("managed"));
    write(&managed, json!({"apiKeyHelper": "/opt/corp/helper"}));
    let user = format!("{}/settings.json", temp_dir("user"));
    write(&user, json!({"env": {"CLAUDE_CODE_USE_BEDROCK": "1", "ANTHROPIC_API_KEY": "x"}}));
    let strict = SettingsBinder { helper_path: "/h".into(), managed_settings_paths: vec![managed], user_settings_path: user };
    let conflicts = strict.environment_conflicts(&root, &BTreeMap::new());
    assert_eq!(conflicts.iter().filter(|c| c.blocking).count(), 2);
    assert_eq!(conflicts.iter().filter(|c| !c.blocking).count(), 1);
    assert!(strict.apply(&sample(), &root, None, false, None).is_err());
    assert!(!Path::new(&local(&root)).exists());
}

#[test]
fn global_profile_is_written_reverted_and_repair_removes_plaintext_keys() {
    let user = format!("{}/settings.json", temp_dir("home"));
    std::fs::write(&user, r#"{"model": "opus", "env": {"ANTHROPIC_API_KEY": "sk-plain", "DEBUG": 1, }, }"#).unwrap();
    let b = SettingsBinder { user_settings_path: user.clone(), ..binder() };
    let backup = b.repair_user_settings().unwrap().unwrap();
    assert!(backup.exists());
    let repaired = read(&user);
    assert!(repaired["env"].get("ANTHROPIC_API_KEY").is_none());
    assert_eq!(repaired["env"]["DEBUG"], "1");
    assert!(repaired["$schema"].is_string());

    let global = b.apply_global(&sample(), None).unwrap();
    assert_eq!(b.inspect_global(&global.binding, &sample()), BindingHealth::Active);
    assert!(b.revert_global(&global.binding).unwrap().is_empty());
    let after = read(&user);
    assert!(after.get("apiKeyHelper").is_none());
    assert_eq!(after["model"], "opus");
}

#[test]
fn subfolders_and_worktrees_resolve_to_main_root_and_git_exclude_is_managed() {
    let repo = temp_dir("repo");
    git_init(&repo);
    std::fs::create_dir_all(format!("{repo}/sub/deep")).unwrap();
    let worktree = canonical_path(&format!("{}/repo wt", Path::new(&repo).parent().unwrap().display()));
    assert!(git(&repo, &["worktree", "add", "-q", "--detach", &worktree]).is_some());
    assert_eq!(settings::settings_root(&format!("{repo}/sub/deep")), repo);
    assert_eq!(settings::settings_root(&worktree), repo);
    assert!(matches!(binder().apply(&sample(), &format!("{repo}/sub"), None, false, None), Err(Error::NotSettingsRoot { .. })));

    let first = binder().apply(&sample(), &repo, None, false, None).unwrap();
    assert_eq!(first.binding.git_exclude_entry.as_deref(), Some(GIT_EXCLUDE_LINE));
    assert!(git(&repo, &["check-ignore", "-q", ".claude/settings.local.json"]).is_some());
    assert_eq!(git(&repo, &["status", "--porcelain"]).as_deref(), Some(""));
    let second = binder().apply(&sample(), &repo, Some(&first.binding), false, None).unwrap();
    let exclude = std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap();
    assert_eq!(exclude.matches(GIT_EXCLUDE_LINE).count(), 1);
    binder().revert(&second.binding).unwrap();
    assert!(!std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap().contains(GIT_EXCLUDE_LINE));
}

#[test]
fn tracked_local_settings_block() {
    let repo = temp_dir("repo");
    git_init(&repo);
    write(&local(&repo), json!({"model": "opus"}));
    assert!(git(&repo, &["add", "-f", ".claude/settings.local.json"]).is_some() && git(&repo, &["commit", "-qm", "add"]).is_some());
    assert!(binder().apply(&sample(), &repo, None, false, None).is_err());
}

#[test]
fn metadata_round_trip_rejects_newer_schema_and_reads_macos_files() {
    let dir = temp_dir("meta");
    let store = MetadataStore::new(Path::new(&dir).join("sub/state.json"), true);
    assert_eq!(store.load().unwrap(), AppState::default());
    let state = AppState { profiles: vec![sample()], bindings: vec![WorkspaceBinding::new("/x".into(), ALPHA.into())], ..AppState::default() };
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), state);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(MetadataStore::decode(br#"{"schemaVersion":99,"profiles":[],"bindings":[]}"#), Err(Error::UnsupportedSchemaVersion(99)));
    assert!(MetadataStore::decode(b"nope").is_err());

    // Written by the macOS app (lowercase ID accepted, normalised).
    let swift = br#"{"bindings":[{"id":"11111111-2222-4333-8444-555555555555","managedValues":{},"path":"\/p","profileID":"6f1e2d3c-4b5a-4978-8695-a4b3c2d1e0f9"}],
        "profiles":[{"credential":{"account":"6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9","service":"KeyZapper.LiteLLM"},"endpoint":"https:\/\/litellm.example.test","id":"6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9","modelAlias":"","name":"Alpha"}],
        "schemaVersion":1}"#;
    let decoded = MetadataStore::decode(swift).unwrap();
    assert_eq!(decoded.bindings[0].profile_id, ALPHA);
    assert_eq!(decoded.bindings[0].path, "/p");
}

fn helper(dir: &str, budgets: BTreeMap<String, f64>) -> HelperCommand {
    HelperCommand {
        metadata: MetadataStore::new(Path::new(dir).join("state.json"), true),
        store: KeyStore { file: Path::new(dir).join("keys.json") },
        tokens: TokenStore { file: Path::new(dir).join("sso-tokens.json") },
        config: ManagedConfig::default(),
        remaining_budget: Box::new(move |_, key| budgets.get(key).copied()),
    }
}

fn args(command: &str, id: &str) -> Vec<String> {
    vec![command.into(), "--profile".into(), id.into()]
}

#[test]
fn an_older_build_dropping_the_office_fields_does_not_lose_them() {
    use keyzapper_core::claude_office::OfficeApplied;
    let dir = temp_dir("mirror");
    let store = MetadataStore::new(Path::new(&dir).join("state.json"), true);
    // One profile with model values (the older check skips those), one without; both carry Office settings.
    let (mut with_models, mut plain) = (sample(), other());
    with_models.office = Some(office_client());
    plain.office = Some(OfficeClient { enabled: false, ..office_client() });
    let state = AppState {
        profiles: vec![with_models, plain],
        office_applied: Some(OfficeApplied { profile_id: ALPHA.into(), addin_id: office_client().addin_id, fingerprint: "f".into(), apps: vec!["excel".into()] }),
        office_error: Some("denied".into()),
        ..AppState::default()
    };
    store.save(&state).unwrap();
    // An older build rewrites state.json without the fields it does not know (the mirror is not touched).
    let mut value: Value = read(&store.path().to_string_lossy());
    value.as_object_mut().unwrap().remove("officeApplied");
    value.as_object_mut().unwrap().remove("officeError");
    for profile in value["profiles"].as_array_mut().unwrap() {
        profile.as_object_mut().unwrap().remove("office");
    }
    write(&store.path().to_string_lossy(), value);
    let raw = MetadataStore::decode(&std::fs::read(store.path()).unwrap()).unwrap();
    assert!(raw.office_applied.is_none() && raw.profiles.iter().all(|p| p.office.is_none()));
    assert_eq!(store.load().unwrap(), state, "the record of the written manifests and the settings come back from the mirror");

    // A removal made by this version is saved to both files, so nothing is brought back.
    let removed = AppState { profiles: vec![sample(), other()], ..AppState::default() };
    store.save(&removed).unwrap();
    assert_eq!(store.load().unwrap(), removed);
}

#[test]
fn an_older_build_dropping_a_tenant_choice_does_not_lose_it() {
    use keyzapper_core::models::{McpServer, McpTransport};
    let dir = temp_dir("mirror-models");
    let store = MetadataStore::new(Path::new(&dir).join("state.json"), true);
    let selection = ModelSelection { tenant: "acme".into(), sonnet_model: Some("acme/claude-sonnet-5-5".into()), ..ModelSelection::default() };
    // Three projects: one with a tenant of its own (and MCP servers, which an older build keeps), one with a tenant that
    // is re-assigned to another profile meanwhile, one without a tenant.
    let mut tenant_project = WorkspaceBinding::new("/a".into(), ALPHA.into());
    tenant_project.models = Some(selection.clone());
    tenant_project.mcp_server_ids = vec![ALPHA.into()];
    let mut moved_project = WorkspaceBinding::new("/b".into(), ALPHA.into());
    moved_project.models = Some(selection.clone());
    let plain_project = WorkspaceBinding::new("/c".into(), ALPHA.into());
    let mut server = McpServer::new(ALPHA.into(), "docs".into());
    server.transport = McpTransport::Http;
    server.url = "https://mcp.example.test/mcp".into();
    let state = AppState {
        profiles: vec![sample(), other()],
        bindings: vec![tenant_project, moved_project, plain_project],
        mcp_servers: vec![server],
        ..AppState::default()
    };
    store.save(&state).unwrap();
    // An older build rewrites state.json without `models` (the mirror is not touched); it knows the MCP fields.
    let file = store.path().to_string_lossy().into_owned();
    let mut value: Value = read(&file);
    for binding in value["bindings"].as_array_mut().unwrap() {
        binding.as_object_mut().unwrap().remove("models");
    }
    value["bindings"][1]["profileID"] = json!(BETA);
    write(&file, value);
    let loaded = store.load().unwrap();
    assert!(!loaded.mcp_servers.is_empty(), "the older build kept the MCP servers, so only `models` is missing");
    assert_eq!(loaded.bindings[0].models.as_ref(), Some(&selection), "the tenant of a project comes back from the mirror");
    assert!(loaded.bindings[1].models.is_none(), "the models of another profile are not given to a project re-assigned meanwhile");
    assert!(loaded.bindings[2].models.is_none());
    assert_eq!(loaded.bindings[0].mcp_server_ids, state.bindings[0].mcp_server_ids);

    // Going back to the profile's own models with this version is saved to both files, so nothing is brought back.
    let mut cleared = state.clone();
    cleared.bindings[0].models = None;
    store.save(&cleared).unwrap();
    assert!(store.load().unwrap().bindings[0].models.is_none());
}

#[test]
fn a_stored_office_object_missing_a_field_still_loads() {
    let dir = temp_dir("office-partial");
    let store = MetadataStore::new(Path::new(&dir).join("state.json"), false);
    let mut profile = sample();
    profile.office = Some(office_client());
    store.save(&AppState { profiles: vec![profile], ..AppState::default() }).unwrap();
    let file = store.path().to_string_lossy().into_owned();
    let full: Value = read(&file);
    for field in ["enabled", "addinId", "gatewayUrl", "entraClientId", "entraScope", "disabledFeatures"] {
        let mut value = full.clone();
        value["profiles"][0]["office"].as_object_mut().unwrap().remove(field);
        write(&file, value);
        let loaded = store.load().unwrap_or_else(|e| panic!("without {field}: {e:?}"));
        let stored = loaded.profiles[0].office.clone().expect("the rest of the office settings is kept");
        // The missing field is empty, everything else is as stored.
        let mut expected = office_client();
        match field {
            "enabled" => expected.enabled = false,
            "addinId" => expected.addin_id.clear(),
            "gatewayUrl" => expected.gateway_url.clear(),
            "entraClientId" => expected.entra_client_id.clear(),
            "entraScope" => expected.entra_scope.clear(),
            _ => expected.disabled_features.clear(),
        }
        assert_eq!(stored, expected, "without {field}");
        // A missing setting is not hidden: the profile is reported as unusable instead of the whole state failing to load.
        let optional = field == "enabled" || field == "disabledFeatures";
        assert_eq!(stored.problems().is_empty(), optional, "without {field}");
    }
}

#[test]
fn helper_returns_only_requested_key_and_never_falls_back() {
    let dir = temp_dir("helper");
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample(), other()], ..AppState::default() }).unwrap();
    assert_eq!(h.run(&args("store", ALPHA), || "sk-alpha\n".into()).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("store", BETA), || "sk-beta".into()).exit_code, ExitCode::Ok);
    let out = h.run(&args("credential", &ALPHA.to_lowercase()), String::new);
    assert_eq!((out.exit_code, out.stdout.as_str()), (ExitCode::Ok, "sk-alpha"));
    assert_eq!(h.run(&args("credential", BETA), String::new).stdout, "sk-beta");
    assert_eq!(h.run(&args("store", ALPHA), || " \n".into()).exit_code, ExitCode::MissingCredential);
    assert_eq!(h.run(&args("status", ALPHA), String::new).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("delete", ALPHA), String::new).exit_code, ExitCode::Ok);
    let missing = h.run(&args("credential", ALPHA), String::new);
    assert_eq!(missing.exit_code, ExitCode::MissingCredential);
    assert!(missing.stdout.is_empty() && !missing.stderr.contains("sk-"));
    assert_eq!(h.run(&args("credential", "6F1E2D3C-0000-4978-8695-A4B3C2D1E0F9"), String::new).exit_code, ExitCode::UnknownProfile);
    assert_eq!(h.run(&args("credential", "not-a-uuid"), String::new).exit_code, ExitCode::UnknownProfile);
    assert_eq!(h.run(&["credential".into()], String::new).exit_code, ExitCode::Usage);
    assert_eq!(h.run(&args("export", BETA), String::new).exit_code, ExitCode::Usage);
}

#[test]
fn hermes_json_static_key_has_no_invented_expiry() {
    let dir = temp_dir("hermes-json-key");
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample()], ..AppState::default() }).unwrap();
    h.store.write(ALPHA, "synthetic-key").unwrap();
    let mut a = args("credential", ALPHA); a.push("--json".into());
    let out = h.run(&a, String::new);
    assert_eq!(out.exit_code, ExitCode::Ok);
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), json!({"access_token":"synthetic-key"}));
    assert!(out.stderr.is_empty());
    assert_eq!(h.run(&args("credential", ALPHA), String::new).stdout, "synthetic-key");
}

#[test]
fn hermes_json_sso_contains_only_matching_credential_and_expiry() {
    let dir = temp_dir("hermes-json-sso");
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sso()], ..AppState::default() }).unwrap();
    let expiry = far_future();
    h.tokens.write(BETA, &TokenSet { refresh_token: "NEVER-OUTPUT".into(), access_token: Some("synthetic-access".into()), expires_at: Some(expiry), token_type: Default::default(), expiry_verified: Some(true) }).unwrap();
    let mut a = args("credential", BETA); a.push("--json".into());
    let out = h.run(&a, String::new);
    assert_eq!(out.exit_code, ExitCode::Ok);
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), json!({"access_token":"synthetic-access", "expiry":keyzapper_core::time::iso8601_utc_at(expiry)}));
    assert!(!out.stdout.contains("NEVER-OUTPUT"));
}

#[test]
fn hermes_json_sso_returns_configured_id_token() {
    let dir = temp_dir("hermes-json-id-token");
    let h = helper(&dir, BTreeMap::new());
    let mut profile = sso();
    profile.oidc_token_type = keyzapper_core::models::OidcTokenType::Id;
    h.metadata.save(&AppState { profiles: vec![profile], ..AppState::default() }).unwrap();
    let expiry = far_future();
    h.tokens.write(BETA, &TokenSet { refresh_token: "DO-NOT-OUTPUT".into(), access_token: Some("configured-id-token".into()), expires_at: Some(expiry), token_type: keyzapper_core::models::OidcTokenType::Id, expiry_verified: Some(true) }).unwrap();
    let mut a = args("credential", BETA); a.push("--json".into());
    let out = h.run(&a, String::new);
    assert_eq!(out.exit_code, ExitCode::Ok);
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), json!({"access_token":"configured-id-token", "expiry":keyzapper_core::time::iso8601_utc_at(expiry)}));
    assert!(!out.stdout.contains("DO-NOT-OUTPUT"));
}

#[test]
fn hermes_json_omits_unverified_sso_expiry_and_rejects_bad_invocations() {
    let dir = temp_dir("hermes-json-unverified");
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample(), sso()], ..AppState::default() }).unwrap();
    h.tokens.write(BETA, &TokenSet { refresh_token: String::new(), access_token: Some("secret-access".into()), expires_at: Some(far_future()), token_type: Default::default(), expiry_verified: None }).unwrap();
    let mut json_args = args("credential", BETA); json_args.push("--json".into());
    let out = h.run(&json_args, String::new);
    assert_eq!(out.exit_code, ExitCode::SessionExpired);
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.contains("secret-access"));
    for bad in [vec!["pool".into(), "--profile".into(), ALPHA.into(), "--json".into()], vec!["credential".into(), "--profile".into(), ALPHA.into(), "--json".into(), "--json".into()], vec!["credential".into(), "--profile".into(), ALPHA.into(), "extra".into()]] {
        let out = h.run(&bad, String::new);
        assert_eq!(out.exit_code, ExitCode::Usage);
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn hermes_json_failures_never_release_a_credential() {
    let dir = temp_dir("hermes-json-failures");
    let h = helper(&dir, BTreeMap::new());
    let mut subscription = sample();
    subscription.id = BETA.into();
    subscription.auth_type = AuthType::Subscription;
    h.metadata.save(&AppState { profiles: vec![sample(), subscription], ..AppState::default() }).unwrap();
    let request = |id: &str| { let mut a = args("credential", id); a.push("--json".into()); a };
    for (id, expected) in [(ALPHA, ExitCode::MissingCredential), ("6F1E2D3C-0000-4978-8695-A4B3C2D1E0F9", ExitCode::UnknownProfile), (BETA, ExitCode::Usage)] {
        let out = h.run(&request(id), String::new);
        assert_eq!(out.exit_code, expected);
        assert!(out.stdout.is_empty());
        assert!(!out.stderr.contains("synthetic"));
    }
    let mut denied = helper(&dir, BTreeMap::new());
    denied.metadata.save(&AppState { profiles: vec![sample()], ..AppState::default() }).unwrap();
    denied.store.write(ALPHA, "synthetic-secret").unwrap();
    denied.config.allowed_gateway_hosts = vec!["other.example".into()];
    let out = denied.run(&request(ALPHA), String::new);
    assert_eq!(out.exit_code, ExitCode::ConfigError);
    assert!(out.stdout.is_empty() && !out.stderr.contains("synthetic-secret"));
    let insecure = helper(&dir, BTreeMap::new());
    let mut http_profile = sample();
    http_profile.endpoint = "http://gateway.example.test".into();
    insecure.metadata.save(&AppState { profiles: vec![http_profile], ..AppState::default() }).unwrap();
    insecure.store.write(ALPHA, "synthetic-secret").unwrap();
    let out = insecure.run(&request(ALPHA), String::new);
    assert_eq!(out.exit_code, ExitCode::ConfigError);
    assert!(out.stdout.is_empty() && !out.stderr.contains("synthetic-secret"));
}

#[test]
fn helper_respects_gateway_allowlist() {
    let dir = temp_dir("helper");
    let mut h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample()], ..AppState::default() }).unwrap();
    h.store.write(ALPHA, "sk-alpha").unwrap();
    h.config.allowed_gateway_hosts = vec!["litellm.firma.example".into()];
    assert_eq!(h.run(&args("credential", ALPHA), String::new).exit_code, ExitCode::ConfigError);
}

#[test]
fn pool_falls_back_to_key_with_most_budget_on_same_endpoint() {
    let dir = temp_dir("pool");
    let gamma = Profile::new("1A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9".into(), "Gamma".into(), "https://litellm.example.test".into(), String::new());
    let delta = Profile::new("2A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9".into(), "Delta".into(), "https://litellm.example.test/".into(), String::new());
    let left = |alpha: f64| BTreeMap::from([("sk-alpha".into(), alpha), ("sk-beta".into(), 100.0), ("sk-gamma".into(), 5.0), ("sk-delta".into(), 8.0)]);
    let setup = |h: &HelperCommand| {
        h.metadata.save(&AppState { profiles: vec![sample(), other(), gamma.clone(), delta.clone()], ..AppState::default() }).unwrap();
        for (id, key) in [(ALPHA, "sk-alpha"), (BETA, "sk-beta"), (gamma.id.as_str(), "sk-gamma"), (delta.id.as_str(), "sk-delta")] {
            h.store.write(id, key).unwrap();
        }
    };
    let h = helper(&dir, left(1.0));
    setup(&h);
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-alpha");
    let h = helper(&dir, left(0.0));
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-delta");
}

fn payload() -> BackupPayload {
    BackupPayload {
        created_at: "2026-09-21T14:13:20Z".into(),
        app_version: Some("1.0.3".into()),
        state: AppState { profiles: vec![sample()], bindings: vec![WorkspaceBinding::new("/p".into(), ALPHA.into())], ..AppState::default() },
        keys: BTreeMap::from([(ALPHA.into(), "sk-secret-alpha".into())]),
    }
}

#[test]
fn backup_round_trip_and_rejections() {
    let password = "correct horse battery";
    let payload = payload();
    let data = backup::seal(&payload, password, 100_000).unwrap();
    assert_eq!(backup::open(&data, password).unwrap(), payload);
    let text = String::from_utf8_lossy(&data);
    assert!(!text.contains("sk-secret") && !text.contains("Alpha"));
    assert_eq!(backup::open(&data, "wrong password!!"), Err(BackupError::WrongPasswordOrCorrupt));
    let tampered = text.replace("100000", "100001");
    assert_eq!(backup::open(tampered.as_bytes(), password), Err(BackupError::WrongPasswordOrCorrupt));
    let hostile = text.replace("100000", "999999999");
    assert_eq!(backup::open(hostile.as_bytes(), password), Err(BackupError::Corrupt));
    assert_eq!(backup::seal(&payload, "short", 100_000), Err(BackupError::PasswordTooShort));
    assert_eq!(backup::open(b"{}", password), Err(BackupError::NotABackup));
}

/// `tests/fixtures/swift-backup.kzbackup` was written by the macOS app (KeyZapper 1.0.7) with the password
/// "correct horse battery" and the payload above.
#[test]
fn opens_backups_made_by_the_macos_app() {
    let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/swift-backup.kzbackup")).unwrap();
    let opened = backup::open(&data, "correct horse battery").unwrap();
    assert_eq!(opened.keys, payload().keys);
    assert_eq!(opened.state.profiles[0].id, ALPHA);
    assert_eq!(opened.created_at, "2026-09-21T14:13:20Z");
}

fn sso() -> Profile {
    let mut p = Profile::new(BETA.into(), "SSO".into(), "https://gw.example.test".into(), String::new());
    p.auth_type = AuthType::Oidc;
    p.oidc_issuer = Some("http://127.0.0.1:1".into());
    p.oidc_client_id = Some("client".into());
    p
}

fn far_future() -> i64 {
    keyzapper_core::oidc::unix_now() + 3600
}

#[test]
fn profiles_without_auth_type_load_as_api_key_and_sso_round_trips() {
    let old: Profile = serde_json::from_value(json!({"id": ALPHA, "name": "A", "endpoint": "https://x.example"})).unwrap();
    assert_eq!(old.auth_type, AuthType::ApiKey);
    assert!(!old.is_sso());
    assert!(serde_json::to_value(&old).unwrap().get("oidcIssuer").is_none());
    let value = serde_json::to_value(sso()).unwrap();
    assert_eq!(value["authType"], "oidc");
    assert_eq!(value["oidcClientId"], "client");
    assert_eq!(serde_json::from_value::<Profile>(value).unwrap(), sso());
}

#[test]
fn token_store_round_trips_and_stays_out_of_keys_file() {
    let dir = temp_dir("tokens");
    let tokens = TokenStore { file: Path::new(&dir).join("sso-tokens.json") };
    assert!(!tokens.exists(BETA).unwrap());
    tokens.write(BETA, &TokenSet { refresh_token: "R".into(), access_token: Some("A".into()), expires_at: Some(5), token_type: Default::default(), expiry_verified: None }).unwrap();
    assert!(tokens.exists(BETA).unwrap());
    assert_eq!(tokens.read(BETA).unwrap().unwrap().access_token.as_deref(), Some("A"));
    assert!(!Path::new(&dir).join("keys.json").exists());
    tokens.delete(BETA).unwrap();
    assert!(tokens.read(BETA).unwrap().is_none());
}

#[test]
fn sso_helper_exit_codes() {
    let dir = temp_dir("sso-helper");
    let mut state = AppState::default();
    state.profiles = vec![sample(), sso()];
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&state).unwrap();
    // No session yet.
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::SessionExpired);
    assert_eq!(h.run(&args("credential", BETA), String::new).exit_code, ExitCode::SessionExpired);
    assert_eq!(ExitCode::SessionExpired as i32, 67);
    // Cached, still valid access token.
    h.tokens.write(BETA, &TokenSet { refresh_token: "R".into(), access_token: Some("jwt-1".into()), expires_at: Some(far_future()), token_type: Default::default(), expiry_verified: None }).unwrap();
    let out = h.run(&args("credential", BETA), String::new);
    assert_eq!((out.exit_code, out.stdout.as_str()), (ExitCode::Ok, "jwt-1"));
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::Ok);
    // pool and store are refused.
    assert_eq!(h.run(&args("pool", BETA), String::new).exit_code, ExitCode::ConfigError);
    assert_eq!(h.run(&args("store", BETA), || "sk-x".into()).exit_code, ExitCode::Usage);
    assert!(!h.store.exists(BETA).unwrap());
    // Gateway allowlist still applies to the endpoint.
    let mut restricted = helper(&dir, BTreeMap::new());
    restricted.config.allowed_gateway_hosts = vec!["other.example".into()];
    assert_eq!(restricted.run(&args("credential", BETA), String::new).exit_code, ExitCode::ConfigError);
    // delete removes the tokens.
    assert_eq!(h.run(&args("delete", BETA), String::new).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::SessionExpired);
}

#[test]
fn pool_skips_sso_profiles() {
    let dir = temp_dir("sso-pool");
    let mut sso_profile = sso();
    sso_profile.endpoint = sample().endpoint;
    let mut state = AppState::default();
    state.profiles = vec![sample(), sso_profile];
    let h = helper(&dir, BTreeMap::from([("sk-own".to_string(), 0.0), ("sk-sso".to_string(), 50.0)]));
    h.metadata.save(&state).unwrap();
    h.store.write(ALPHA, "sk-own").unwrap();
    h.store.write(BETA, "sk-sso").unwrap(); // stray key on an SSO profile must not be pooled
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-own");
}

#[test]
fn sso_profiles_are_never_bound_with_pool() {
    let root = temp_dir("sso-proj");
    let result = binder().apply(&sso(), &root, None, true, None).unwrap();
    assert_eq!(result.binding.pooled, None);
    let helper_cmd = read(&local(&root))["apiKeyHelper"].as_str().unwrap().to_string();
    assert!(helper_cmd.ends_with(&format!("credential --profile {BETA}")));
    assert!(read(&local(&root))["env"].get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").is_none());
}

#[test]
fn backup_keeps_sso_metadata_but_never_tokens() {
    let dir = temp_dir("sso-backup");
    let tokens = TokenStore { file: Path::new(&dir).join("sso-tokens.json") };
    tokens.write(BETA, &TokenSet { refresh_token: "refresh-secret".into(), access_token: Some("access-secret".into()), expires_at: Some(1), token_type: Default::default(), expiry_verified: None }).unwrap();
    let mut payload = payload();
    payload.state.profiles.push(sso());
    let plain = serde_json::to_string(&payload).unwrap();
    assert!(!plain.contains("refresh-secret") && !plain.contains("access-secret"));
    let opened = backup::open(&backup::seal(&payload, "correct horse battery", 100_000).unwrap(), "correct horse battery").unwrap();
    let restored = opened.state.profile(BETA).unwrap();
    assert_eq!((restored.auth_type, restored.oidc_issuer.as_deref(), restored.oidc_client_id.as_deref()), (AuthType::Oidc, Some("http://127.0.0.1:1"), Some("client")));
    assert!(!opened.keys.contains_key(BETA));
}

// MARK: Agent instruction files

#[test]
fn agents_scan_counts_languages_and_skips_dependencies() {
    use keyzapper_core::agents;
    let root = temp_dir("repo");
    for (file, text) in [
        ("src/main.rs", "fn main() {}"),
        ("src/lib.rs", ""),
        ("ui/app.ts", ""),
        ("Cargo.toml", "[package]\nname = \"demo\""),
        ("README.md", "# Demo\nA demo project."),
        ("CLAUDE.md", "Run cargo test."),
        ("node_modules/left-pad/index.js", ""),
        ("target/debug/build.rs", ""),
    ] {
        let path = Path::new(&root).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let scan = agents::scan(&root);
    assert!(!scan.git);
    assert_eq!(scan.file_count, 6);
    assert_eq!(scan.languages, vec![("Rust".to_string(), 2), ("TypeScript".to_string(), 1)]);
    assert_eq!(scan.manifests, vec!["Cargo.toml"]);
    assert_eq!(scan.agent_files, vec!["CLAUDE.md"]);
    assert!(!scan.has_agents_file);

    let context = agents::context(&scan);
    assert!(context.contains("src/ (2 files)"));
    assert!(context.contains("=== Cargo.toml ===\n[package]"));
    assert!(context.contains("=== Existing CLAUDE.md ===\nRun cargo test."));
    assert!(!context.contains("node_modules"));
}

#[test]
fn agents_reply_is_parsed_into_proposal() {
    use keyzapper_core::agents;
    let reply = "<assessment>\nA Rust CLI.\n</assessment>\n<recommendations>\ntask | model | tier | reason\n|---|---|---|---|\n\
- Default | `team-sonnet` | Sonnet | Everyday work\n| Search | team-haiku | haiku | Cheap |\nnot a row\n</recommendations>\n\
<agents_md>\n```markdown\n# AGENTS.md\n\n- Run `cargo test`.\n```\n</agents_md>";
    let proposal = agents::parse_reply(reply).unwrap();
    assert_eq!(proposal.assessment, "A Rust CLI.");
    assert_eq!(proposal.content, "# AGENTS.md\n\n- Run `cargo test`.");
    let rows: Vec<(&str, &str, &str)> = proposal.recommendations.iter().map(|r| (r.task.as_str(), r.model.as_str(), r.tier.as_str())).collect();
    assert_eq!(rows, vec![("Default", "team-sonnet", "sonnet"), ("Search", "team-haiku", "haiku")]);
    assert_eq!(agents::parse_reply("<assessment>No file</assessment>"), None);
}

#[test]
fn agents_analysis_model_prefers_choice_then_profile_then_gateway() {
    use keyzapper_core::agents::analysis_model;
    let profile = sample();
    assert_eq!(analysis_model(" chosen ", &profile, &[]).as_deref(), Some("chosen"));
    assert_eq!(analysis_model("", &profile, &[]).as_deref(), Some("team-opus"));
    let bare = other();
    let available = vec!["claude-haiku-5-5".to_string(), "claude-sonnet-5-5".to_string()];
    assert_eq!(analysis_model("", &bare, &available).as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(analysis_model("", &bare, &[]), None);
}

#[test]
fn agents_prompt_lists_goals_and_models() {
    use keyzapper_core::agents::{prompt, Goal, Options};
    let options = Options { goals: vec![Goal::Cost, Goal::Security], language: "de".into(), model: String::new(), notes: "Use pnpm.".into() };
    let text = prompt("CONTEXT", &options, &sample(), &["team-opus".to_string(), "team-haiku".to_string()]);
    assert!(text.contains("<repository>\nCONTEXT\n</repository>"));
    assert!(text.contains("- Cost efficiency:") && text.contains("- Security:") && !text.contains("- Speed:"));
    assert!(text.contains("- opus: team-opus\n- default (ANTHROPIC_MODEL): claude-alpha"));
    assert!(text.contains("on the gateway: team-haiku\n"));
    assert!(text.contains("Use pnpm.") && text.contains("AGENTS.md in German"));
}

/// Answers one HTTP request with `status` and `body`; returns the endpoint and the raw request it received.
fn one_shot_server(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            request.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&request);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (endpoint, handle)
}

#[test]
fn gateway_complete_posts_messages_and_reads_text() {
    use keyzapper_core::gateway::complete;
    use std::time::Duration;
    let (endpoint, server) = one_shot_server("200 OK",
        r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":12,"output_tokens":3}}"#);
    let reply = complete(&format!("{endpoint}/"), "sk-test", "team-opus", "SYSTEM", "PROMPT", 100, Duration::from_secs(5)).unwrap();
    assert_eq!((reply.text.as_str(), reply.input_tokens, reply.output_tokens), ("Hello", 12, 3));
    let request = server.join().unwrap().to_lowercase();
    assert!(request.starts_with("post /v1/messages http/1.1"));
    assert!(request.contains("x-api-key: sk-test") && request.contains("authorization: bearer sk-test") && request.contains("anthropic-version: 2023-06-01"));
    assert!(request.contains(r#""model":"team-opus""#) && request.contains(r#""system":"system""#));

    let (endpoint, server) = one_shot_server("400 Bad Request", r#"{"error":{"message":"Invalid model name"}}"#);
    let error = complete(&endpoint, "sk-test", "nope", "S", "P", 100, Duration::from_secs(5)).err().unwrap();
    server.join().unwrap();
    assert!(error.contains("400") && error.contains("Invalid model name"), "{error}");
}

#[test]
fn gateway_sso_budget_falls_back_to_the_user() {
    use keyzapper_core::gateway::sso_budget;
    use std::io::{Read, Write};
    use std::time::Duration;
    // A JWT without a mapped virtual key: `/key/info` finds no key, `/user/info` has the user's budget.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let replies = [("404 Not Found", r#"{"error":{"message":"Key not found in database"}}"#),
            ("200 OK", r#"{"user_id":"u1","user_info":{"spend":12.5,"max_budget":50.0,"budget_reset_at":"2026-11-01T00:00:00"},"keys":[],"teams":[]}"#)];
        replies.map(|(status, body)| {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let n = stream.read(&mut buffer).unwrap();
            write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&buffer[..n]).lines().next().unwrap_or_default().to_lowercase()
        })
    });
    let budget = sso_budget(&endpoint, "jwt", None, Duration::from_secs(5)).unwrap();
    assert_eq!((budget.spend, budget.max_budget, budget.reset_at.as_deref()), (12.5, Some(50.0), Some("2026-11-01T00:00:00")));
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("get /key/info ") && requests[1].starts_with("get /user/info "), "{requests:?}");
}

/// Fake gateway: answers each request with the next `(status, body)` and returns the request lines, lowercased.
fn budget_server(replies: Vec<(&'static str, &'static str)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        replies
            .into_iter()
            .map(|(status, body)| {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let n = stream.read(&mut buffer).unwrap();
                write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
                String::from_utf8_lossy(&buffer[..n]).lines().next().unwrap_or_default().to_lowercase()
            })
            .collect()
    });
    (endpoint, server)
}

#[test]
fn gateway_sso_budget_asks_for_the_tenant_of_the_model() {
    use keyzapper_core::gateway::sso_budget;
    use std::time::Duration;
    // The tenant's access has no mapped key either: `/key/info?model=` fails, `/user/info?model=` answers.
    let (endpoint, server) = budget_server(vec![
        ("404 Not Found", r#"{"error":{"message":"Key not found in database"}}"#),
        ("200 OK", r#"{"user_id":"u1","user_info":{"spend":3.0,"max_budget":50.0,"budget_reset_at":null},"keys":[],"teams":[]}"#),
    ]);
    let budget = sso_budget(&endpoint, "jwt", Some("acme/claude-sonnet-5-5"), Duration::from_secs(5)).unwrap();
    assert_eq!(budget.max_budget, Some(50.0));
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("get /key/info?model=acme%2fclaude-sonnet-5-5 "), "{requests:?}");
    assert!(requests[1].starts_with("get /user/info?model=acme%2fclaude-sonnet-5-5 "), "{requests:?}");
}

#[test]
fn gateway_budget_encodes_the_model_and_skips_an_empty_one() {
    use keyzapper_core::gateway::budget;
    use std::time::Duration;
    let body = r#"{"info":{"spend":1.0,"max_budget":10.0,"budget_reset_at":null}}"#;
    let (endpoint, server) = budget_server(vec![("200 OK", body), ("200 OK", body), ("200 OK", body)]);
    for model in [Some("a b/c&d=e"), Some(""), None] {
        assert_eq!(budget(&endpoint, "sk-test", model, Duration::from_secs(5)).unwrap().max_budget, Some(10.0));
    }
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("get /key/info?model=a+b%2fc%26d%3de "), "{requests:?}");
    assert!(requests[1].starts_with("get /key/info ") && requests[2].starts_with("get /key/info "), "{requests:?}");
}

#[test]
fn whats_new_lists_releases_since_the_previous_version() {
    use keyzapper_core::whats_new::since;
    let versions = |previous: Option<&str>, current: &str| since(previous, current).into_iter().map(|r| r.version).collect::<Vec<_>>();
    assert_eq!(versions(Some("1.3.0"), "1.4.0"), vec!["1.4.0"]);
    assert_eq!(versions(Some("1.2.0"), "1.4.0"), vec!["1.4.0", "1.3.0"]);
    assert_eq!(versions(None, "1.4.0"), vec!["1.4.0", "1.3.0"]);
    assert_eq!(versions(Some("1.3.0"), "1.4.0-beta.1"), vec!["1.4.0"]);
    // Betas of one release share its notes: nothing new between them or from the last beta to the release.
    assert!(versions(Some("1.4.0-beta.1"), "1.4.0-beta.2").is_empty());
    assert!(versions(Some("1.4.0-beta.2"), "1.4.0").is_empty());
    assert_eq!(versions(Some("1.4.0"), "1.4.1"), vec!["1.4.1"]);
    assert!(versions(Some("1.4.1"), "1.4.2").is_empty());
    assert_eq!(versions(Some("1.4.2"), "1.4.3"), vec!["1.4.3"]);
    assert_eq!(versions(Some("1.4.1"), "1.4.3"), vec!["1.4.3"]);
    assert!(versions(Some("1.4.3"), "1.4.4").is_empty());
    assert_eq!(versions(Some("1.4.3"), "1.5.0-beta.1"), vec!["1.5.0"]);
    assert!(versions(Some("1.5.0-beta.1"), "1.5.0").is_empty());
    assert_eq!(versions(Some("1.5.0"), "1.5.1"), vec!["1.5.1"]);
    assert_eq!(versions(Some("1.5.1"), "1.5.3"), vec!["1.5.3"]);
    assert!(since(Some("1.3.0"), "1.4.0")[0].items.iter().all(|i| !i.is_empty()));
}

fn mcp_file(root: &str) -> String {
    format!("{root}/.mcp.json")
}

#[test]
fn mcp_servers_are_written_enabled_and_reverted_without_touching_foreign_entries() {
    use keyzapper_core::mcp;
    use keyzapper_core::models::{McpServer, McpTransport};
    let repo = temp_dir("repo");
    git_init(&repo);
    write(&mcp_file(&repo), json!({"mcpServers": {"team": {"type": "http", "url": "https://team.example.test/mcp"}}}));
    write(&local(&repo), json!({"enabledMcpjsonServers": ["team"]}));
    let mut github = McpServer::new(ALPHA.into(), "github".into());
    github.command = "npx".into();
    github.args = vec!["-y".into(), "server-github".into()];
    github.env = BTreeMap::from([("GITHUB_TOKEN".to_string(), "${GITHUB_TOKEN}".to_string())]);
    let mut docs = McpServer::new(BETA.into(), "docs".into());
    docs.transport = McpTransport::Http;
    docs.url = "https://docs.example.test/mcp".into();
    docs.headers = BTreeMap::from([("Authorization".to_string(), "Bearer ${DOCS_TOKEN}".to_string())]);
    docs.oauth_client_id = "weave-claude".into();
    docs.oauth_callback_port = Some(8976);
    docs.oauth_scopes = "mcp.read".into();

    let first = mcp::apply(&repo, &[github.clone(), docs.clone()], &BTreeMap::new(), None).unwrap();
    assert!(first.changed);
    assert_eq!(first.git_exclude_entry.as_deref(), Some(mcp::GIT_EXCLUDE_LINE));
    let file = read(&mcp_file(&repo));
    assert_eq!(file["mcpServers"]["github"], github.definition());
    assert_eq!(github.definition_for(false), json!({"type": "stdio", "command": "npx", "args": ["-y", "server-github"], "env": {"GITHUB_TOKEN": "${GITHUB_TOKEN}"}}));
    assert_eq!(github.definition_for(true), json!({"type": "stdio", "command": "cmd", "args": ["/c", "npx", "-y", "server-github"], "env": {"GITHUB_TOKEN": "${GITHUB_TOKEN}"}}));
    let mut native = github.clone();
    for (command, wrapped) in [("NPX.CMD", true), (r"C:\tools\run.bat", true), ("uvx", false), ("node.exe", false), ("cmd", false)] {
        native.command = command.into();
        assert_eq!(native.definition_for(true)["args"][0] == "/c", wrapped, "{command}");
    }
    assert_eq!(file["mcpServers"]["docs"], json!({"type": "http", "url": "https://docs.example.test/mcp", "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"},
        "oauth": {"clientId": "weave-claude", "callbackPort": 8976, "scopes": "mcp.read"}}));
    assert_eq!(file["mcpServers"]["team"]["url"], "https://team.example.test/mcp");
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team", "docs", "github"]));
    assert!(git(&repo, &["check-ignore", "-q", ".mcp.json"]).is_some());
    assert!(!git(&repo, &["status", "--porcelain"]).unwrap().contains(".mcp.json"));
    assert!(mcp::is_active(&repo, &first.managed));

    let second = mcp::apply(&repo, &[github.clone(), docs.clone()], &first.managed, first.git_exclude_entry.as_deref()).unwrap();
    assert!(!second.changed);
    assert_eq!(second.managed, first.managed);

    // Deselecting a server removes only its entry.
    let third = mcp::apply(&repo, &[github.clone()], &second.managed, second.git_exclude_entry.as_deref()).unwrap();
    assert!(third.changed);
    let file = read(&mcp_file(&repo));
    assert!(file["mcpServers"].get("docs").is_none() && file["mcpServers"].get("team").is_some());
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team", "github"]));

    // A foreign definition under the same name blocks; revert keeps it and reports it.
    write(&mcp_file(&repo), json!({"mcpServers": {"github": {"type": "http", "url": "https://other.example.test"}}}));
    assert!(matches!(mcp::apply(&repo, &[github.clone()], &BTreeMap::new(), None), Err(Error::SettingsConflicts(_))));
    assert!(!mcp::is_active(&repo, &third.managed));
    let kept = mcp::revert(&repo, &third.managed, third.git_exclude_entry.as_deref()).unwrap();
    assert_eq!(kept, vec!["github".to_string()]);
    assert_eq!(read(&mcp_file(&repo))["mcpServers"]["github"]["url"], "https://other.example.test");
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team"]));
    assert!(!std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap().contains(mcp::GIT_EXCLUDE_LINE));

    // State round trip keeps servers and assignments.
    let mut binding = WorkspaceBinding::new(repo.clone(), ALPHA.into());
    binding.mcp_server_ids = vec![github.id.clone()];
    binding.mcp_managed = third.managed.clone();
    let state = AppState { profiles: vec![sample()], bindings: vec![binding], mcp_servers: vec![github, docs], ..AppState::default() };
    assert_eq!(MetadataStore::decode(&serde_json::to_vec(&state).unwrap()).unwrap(), state);
}

#[test]
fn mcp_files_created_for_servers_vanish_on_revert_and_tracked_mcp_json_blocks() {
    use keyzapper_core::mcp;
    use keyzapper_core::models::McpServer;
    let repo = temp_dir("repo");
    git_init(&repo);
    let mut server = McpServer::new(ALPHA.into(), "fs".into());
    server.command = "mcp-fs".into();
    let first = mcp::apply(&repo, &[server.clone()], &BTreeMap::new(), None).unwrap();
    assert!(Path::new(&mcp_file(&repo)).exists() && Path::new(&local(&repo)).exists());
    assert!(mcp::revert(&repo, &first.managed, first.git_exclude_entry.as_deref()).unwrap().is_empty());
    assert!(!Path::new(&mcp_file(&repo)).exists() && !Path::new(&local(&repo)).exists());
    assert!(Path::new(&repo).is_dir());

    write(&mcp_file(&repo), json!({"mcpServers": {}}));
    assert!(git(&repo, &["add", ".mcp.json"]).is_some() && git(&repo, &["commit", "-qm", "mcp"]).is_some());
    assert!(matches!(mcp::apply(&repo, &[server], &BTreeMap::new(), None), Err(Error::SettingsConflicts(_))));
}

#[test]
fn subscription_profile_neutralises_gateway_and_switches_back() {
    let root = temp_dir("proj");
    let mut subscription = Profile::new(BETA.into(), "Abo".into(), String::new(), "opus".into());
    subscription.auth_type = AuthType::Subscription;
    let keyed = binder().apply(&sample(), &root, None, true, None).unwrap();
    let applied = binder().apply(&subscription, &root, Some(&keyed.binding), true, None).unwrap();
    assert_eq!(applied.binding.pooled, None);
    let settings = read(&local(&root));
    assert_eq!(settings["apiKeyHelper"], "");
    for key in ["ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL"] {
        assert_eq!(settings["env"][key], "", "{key}");
    }
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "opus");
    assert!(settings["env"].get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").is_none());
    assert_eq!(binder().inspect(&applied.binding, &subscription, &BTreeMap::new()).health, BindingHealth::Active);
    let back = binder().apply(&sample(), &root, Some(&applied.binding), false, None).unwrap();
    assert!(read(&local(&root))["apiKeyHelper"].as_str().unwrap().ends_with(&format!("credential --profile {ALPHA}")));
    binder().revert(&back.binding).unwrap();
    assert!(!Path::new(&local(&root)).exists());
}

#[test]
fn reads_claude_login_and_plan() {
    use keyzapper_core::subscription::{parse_login, plan_name};
    let login = parse_login(r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat01-x", "refreshToken": "r", "expiresAt": 1760000000000,
        "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"}}"#).unwrap();
    assert_eq!(login.access_token, "sk-ant-oat01-x");
    assert_eq!(login.expires_at, Some(1_760_000_000_000));
    assert_eq!(login.plan.as_deref(), Some("Max 20×"));
    assert_eq!(plan_name(Some("pro"), None).as_deref(), Some("Pro"));
    assert_eq!(plan_name(Some("team"), Some("default_claude_team")).as_deref(), Some("Team"));
    assert!(parse_login(r#"{"claudeAiOauth": {"accessToken": ""}}"#).is_none());
    assert!(parse_login("not json").is_none());
}

#[test]
fn parses_usage_limits_in_both_formats() {
    use keyzapper_core::subscription::parse_usage;
    let listed = parse_usage(r#"{"five_hour": null, "limits": [
        {"kind": "session", "percent": 35.0, "resets_at": "2026-10-19T12:00:00+00:00"},
        {"kind": "weekly_all", "percent": 14},
        {"kind": "weekly_scoped", "percent": 120, "resets_at": null, "scope": {"model": {"display_name": "Opus"}}},
        {"kind": "something_new", "percent": 1}]}"#).unwrap();
    assert_eq!(listed.iter().map(|l| (l.kind.as_str(), l.model.as_deref(), l.percent)).collect::<Vec<_>>(),
        vec![("session", None, 35.0), ("weekly", None, 14.0), ("model", Some("Opus"), 100.0)]);
    assert_eq!(listed[0].resets_at.as_deref(), Some("2026-10-19T12:00:00+00:00"));
    let flat = parse_usage(r#"{"five_hour": {"utilization": 6.0, "resets_at": "2026-10-19T04:59:59+00:00"},
        "seven_day": {"utilization": 35.0, "resets_at": null}, "seven_day_opus": null,
        "seven_day_sonnet": {"utilization": 2.5, "resets_at": null}, "extra_usage": {"is_enabled": false}}"#).unwrap();
    assert_eq!(flat.iter().map(|l| (l.kind.as_str(), l.model.as_deref(), l.percent)).collect::<Vec<_>>(),
        vec![("session", None, 6.0), ("weekly", None, 35.0), ("model", Some("Sonnet"), 2.5)]);
    assert!(parse_usage("[]").is_none());
}

// MARK: Models of one tenant per project (one SSO profile, several tenants)

#[test]
fn tenants_are_the_prefixes_of_gateway_model_ids() {
    use keyzapper_core::models::tenants_of;
    let ids = ["acme/claude-opus-5-5", "globex/claude-sonnet-5-5", "acme/claude-sonnet-5-5", "claude-haiku-5-5"].map(String::from);
    assert_eq!(tenants_of(&ids), vec!["acme", "globex"]);
    assert!(tenants_of(&["claude-haiku-5-5".to_string(), "/orphan".to_string()]).is_empty());
}

#[test]
fn selection_moves_the_profile_tiers_to_another_tenant() {
    use keyzapper_core::models::selection_for_tenant;
    let ids = ["acme/claude-opus-5-5", "acme/claude-sonnet-5-5", "globex/claude-opus-5-5", "globex/claude-sonnet-5-5", "globex/claude-haiku-5-5"].map(String::from);
    let mut profile = sample();
    profile.opus_model = Some("acme/claude-opus-5-5".into());
    profile.sonnet_model = Some("acme/claude-sonnet-5-5".into());
    profile.model_alias = "sonnet".into();
    let s = selection_for_tenant(&ids, "globex", &profile).unwrap();
    assert_eq!(s.tenant, "globex");
    assert_eq!(s.opus_model.as_deref(), Some("globex/claude-opus-5-5"));
    assert_eq!(s.sonnet_model.as_deref(), Some("globex/claude-sonnet-5-5"));
    assert_eq!(s.haiku_model.as_deref(), Some("globex/claude-haiku-5-5"));
    assert_eq!(s.model_alias, "sonnet");
    assert!(selection_for_tenant(&ids, "unknown", &profile).is_none());
}

#[test]
fn selection_for_a_managed_profile_without_tiers_suggests_within_the_tenant() {
    use keyzapper_core::models::selection_for_tenant;
    let ids = ["acme/claude-opus-5-5", "acme/claude-sonnet-5-5", "globex/claude-sonnet-5-5"].map(String::from);
    let s = selection_for_tenant(&ids, "acme", &sample()).unwrap();
    assert_eq!((s.opus_model.as_deref(), s.sonnet_model.as_deref(), s.haiku_model.as_deref()), (Some("acme/claude-opus-5-5"), Some("acme/claude-sonnet-5-5"), None));
}

#[test]
fn a_tenant_named_like_a_tier_does_not_confuse_the_suggestion() {
    use keyzapper_core::models::selection_for_tenant;
    let ids = ["opus-lab/claude-sonnet-5-5", "opus-lab/claude-haiku-5-5"].map(String::from);
    let s = selection_for_tenant(&ids, "opus-lab", &other()).unwrap();
    assert_eq!((s.opus_model.as_deref(), s.sonnet_model.as_deref(), s.haiku_model.as_deref()), (None, Some("opus-lab/claude-sonnet-5-5"), Some("opus-lab/claude-haiku-5-5")));
}

#[test]
fn selection_maps_bare_profile_names_and_aliases() {
    use keyzapper_core::models::selection_for_tenant;
    let ids = ["a/opus-x", "a/sonnet-x", "a/haiku-x", "b/opus-x", "b/sonnet-y"].map(String::from);
    let mut profile = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test".into(), String::new());
    profile.opus_model = Some("opus-x".into()); // bare name: moves to the tenant when it exists there
    profile.sonnet_model = Some("a/sonnet-x".into()); // "b/sonnet-x" does not exist: the tenant's best Sonnet is used
    profile.haiku_model = Some("a/haiku-x".into()); // "b" has no Haiku at all
    let s = selection_for_tenant(&ids, "b", &profile).unwrap();
    assert_eq!((s.opus_model.as_deref(), s.sonnet_model.as_deref(), s.haiku_model.as_deref()), (Some("b/opus-x"), Some("b/sonnet-y"), None));

    // Empty stays empty; the tier aliases (also with [1m]) stay; a model ID moves along or falls back to the Sonnet tier.
    for (alias, expected) in [("", ""), ("opus", "opus"), ("sonnet[1m]", "sonnet[1m]"), ("opusplan", "opusplan"), ("haiku", "haiku"), ("opus[1m]", "opus[1m]")] {
        profile.model_alias = alias.into();
        assert_eq!(selection_for_tenant(&ids, "b", &profile).unwrap().model_alias, expected, "alias {alias:?}");
    }
    profile.model_alias = "a/opus-x".into();
    assert_eq!(selection_for_tenant(&ids, "b", &profile).unwrap().model_alias, "b/opus-x");
    profile.model_alias = "a/unknown-model".into();
    assert_eq!(selection_for_tenant(&ids, "b", &profile).unwrap().model_alias, "b/sonnet-y");
}

#[test]
fn budget_model_prefers_sonnet_then_opus_then_haiku() {
    let mut s = ModelSelection { tenant: "t".into(), haiku_model: Some("t/h".into()), ..Default::default() };
    assert_eq!(s.budget_model(), Some("t/h"));
    s.opus_model = Some("t/o".into());
    assert_eq!(s.budget_model(), Some("t/o"));
    s.sonnet_model = Some("t/s".into());
    assert_eq!(s.budget_model(), Some("t/s"));
    assert_eq!(ModelSelection::default().budget_model(), None);
}

fn tenant_binding(profile_id: &str, tenant: &str, sonnet: Option<&str>) -> WorkspaceBinding {
    let mut binding = WorkspaceBinding::new(format!("/p/{tenant}"), profile_id.into());
    binding.models = Some(ModelSelection { tenant: tenant.into(), sonnet_model: sonnet.map(String::from), ..Default::default() });
    binding
}

#[test]
fn budget_targets_of_a_key_profile_is_one_request_without_model() {
    use keyzapper_core::models::{budget_targets, BudgetTarget};
    let bindings = vec![tenant_binding(ALPHA, "a", Some("a/s"))];
    assert_eq!(budget_targets(&sample(), &bindings), vec![BudgetTarget { tenant: None, model: None }]);
}

#[test]
fn budget_targets_of_an_sso_profile_has_one_request_per_tenant_and_one_for_the_profile() {
    use keyzapper_core::models::{budget_targets, BudgetTarget};
    let mut profile = sample();
    profile.auth_type = AuthType::Oidc;
    let mut without_models = WorkspaceBinding::new("/p/none".into(), ALPHA.into());
    without_models.models = None;
    let bindings = vec![
        tenant_binding(ALPHA, "b", Some("b/s1")),
        tenant_binding(ALPHA, "a", Some("a/s")),
        tenant_binding(ALPHA, "b", Some("b/s2")), // the same tenant again: still one request
        tenant_binding(BETA, "c", Some("c/s")),   // another profile
        without_models,                           // as in the profile
        tenant_binding(ALPHA, "d", None),         // a tenant without any model cannot be asked
    ];
    let target = |tenant: Option<&str>, model: &str| BudgetTarget { tenant: tenant.map(String::from), model: Some(model.into()) };
    // The profile's own models: its Opus model "team-opus" (no Sonnet), not the alias "claude-alpha".
    assert_eq!(budget_targets(&profile, &bindings), vec![target(Some("a"), "a/s"), target(Some("b"), "b/s1"), target(None, "team-opus")]);
}

#[test]
fn profile_budget_model_prefers_sonnet_then_opus_then_haiku_then_a_real_default_model() {
    let mut profile = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test".into(), String::new());
    assert_eq!(profile.budget_model(), None);
    // The default model only counts when it is a model name, not one of Claude Code's tier aliases.
    for alias in ["opus", "Sonnet", "haiku", "opusplan", "sonnet[1m]", "opus[1M]", "  "] {
        profile.model_alias = alias.into();
        assert_eq!(profile.budget_model(), None, "alias {alias:?}");
    }
    profile.model_alias = " claude-x ".into();
    assert_eq!(profile.budget_model(), Some("claude-x"));
    profile.haiku_model = Some("h".into());
    assert_eq!(profile.budget_model(), Some("h"));
    profile.opus_model = Some(" o ".into());
    assert_eq!(profile.budget_model(), Some("o"));
    profile.sonnet_model = Some("s".into());
    assert_eq!(profile.budget_model(), Some("s"));
    profile.sonnet_model = Some("  ".into());
    assert_eq!(profile.budget_model(), Some("o"));
}

#[test]
fn budget_targets_of_an_alias_only_sso_profile_keep_their_budget_without_model() {
    use keyzapper_core::models::{budget_targets, BudgetTarget};
    // A tier alias is no gateway ID: `?model=opus` would be refused, so the budget is asked for without a model.
    for alias in ["opus", "haiku", "sonnet[1m]"] {
        let mut profile = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test".into(), alias.into());
        profile.auth_type = AuthType::Oidc;
        assert_eq!(budget_targets(&profile, &[]), vec![BudgetTarget { tenant: None, model: None }], "alias {alias:?}");
    }
}

#[test]
fn budget_targets_of_an_sso_profile_fall_back_to_a_request_without_model() {
    use keyzapper_core::models::{budget_targets, BudgetTarget};
    let mut profile = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test".into(), String::new());
    profile.auth_type = AuthType::Oidc;
    // No model anywhere: as before, the budget of the signed-in user is asked for without `?model=`.
    assert_eq!(budget_targets(&profile, &[]), vec![BudgetTarget { tenant: None, model: None }]);
    // Tenants without models of the profile: only the tenants, no extra request without model.
    let targets = budget_targets(&profile, &[tenant_binding(ALPHA, "a", Some("a/s"))]);
    assert_eq!(targets, vec![BudgetTarget { tenant: Some("a".into()), model: Some("a/s".into()) }]);
}

#[test]
fn budget_targets_do_not_ask_twice_for_the_same_tenant() {
    use keyzapper_core::models::{budget_targets, BudgetTarget};
    let mut profile = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test".into(), String::new());
    profile.auth_type = AuthType::Oidc;
    profile.opus_model = Some("acme/claude-opus-5".into());
    profile.sonnet_model = Some("acme/claude-sonnet-5-5".into());
    // The profile's own model belongs to tenant "acme", which a project already uses (with another model): the tenant's bar
    // shows that budget, so the profile gets no second one. The tenant is compared by prefix, not the model string.
    let targets = budget_targets(&profile, &[tenant_binding(ALPHA, "acme", Some("acme/claude-sonnet-5-6"))]);
    assert_eq!(targets, vec![BudgetTarget { tenant: Some("acme".into()), model: Some("acme/claude-sonnet-5-6".into()) }]);
    // Another tenant's project: both are shown, the profile entry asks for its Sonnet model (not Opus).
    let targets = budget_targets(&profile, &[tenant_binding(ALPHA, "globex", Some("globex/claude-sonnet-5-5"))]);
    assert_eq!(
        targets,
        vec![
            BudgetTarget { tenant: Some("globex".into()), model: Some("globex/claude-sonnet-5-5".into()) },
            BudgetTarget { tenant: None, model: Some("acme/claude-sonnet-5-5".into()) },
        ]
    );
}

#[test]
fn model_environment_for_replaces_only_the_model_values() {
    let mut profile = sample();
    profile.sonnet_model = Some("team-sonnet".into());
    assert_eq!(profile.model_environment_for(None), profile.model_environment());
    let s = ModelSelection { tenant: "t".into(), sonnet_model: Some("t/s".into()), ..Default::default() };
    // Same keys as `model_environment`, only the selection's values: the profile's alias and Opus model are gone.
    assert_eq!(profile.model_environment_for(Some(&s)), BTreeMap::from([("ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(), "t/s".to_string())]));
}

#[test]
fn two_projects_share_one_sso_profile_with_different_tenants() {
    // Two bindings of the same profile: same helper and base URL, only the model values differ.
    let binder = binder();
    let profile = sample();
    let (a, b) = (temp_dir("tenant-a"), temp_dir("tenant-b"));
    let sel = |t: &str| ModelSelection { tenant: t.into(), sonnet_model: Some(format!("{t}/claude-sonnet-5-5")), ..Default::default() };
    let ra = binder.apply(&profile, &a, None, false, Some(&sel("acme"))).unwrap();
    let rb = binder.apply(&profile, &b, None, false, Some(&sel("globex"))).unwrap();
    let (va, vb) = (read(&local(&a)), read(&local(&b)));
    assert_eq!(va["apiKeyHelper"], vb["apiKeyHelper"]);
    assert_eq!(va["env"]["ANTHROPIC_BASE_URL"], vb["env"]["ANTHROPIC_BASE_URL"]);
    assert_eq!(va["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"], "acme/claude-sonnet-5-5");
    assert_eq!(vb["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"], "globex/claude-sonnet-5-5");
    assert_eq!(ra.binding.models.as_ref().map(|m| m.tenant.as_str()), Some("acme"));
    assert_eq!(binder.inspect(&rb.binding, &profile, &BTreeMap::new()).health, BindingHealth::Active);
}

#[test]
fn switching_the_tenant_changes_only_the_model_keys_and_reverts_cleanly() {
    let binder = binder();
    let profile = sample();
    let root = temp_dir("tenant-switch");
    let sel = |t: &str| ModelSelection { tenant: t.into(), opus_model: Some(format!("{t}/opus")), sonnet_model: Some(format!("{t}/sonnet")), ..Default::default() };
    let first = binder.apply(&profile, &root, None, false, Some(&sel("a"))).unwrap();
    let before = read(&local(&root));
    let second = binder.apply(&profile, &root, Some(&first.binding), false, Some(&sel("b"))).unwrap();
    assert!(second.changed);
    let after = read(&local(&root));
    assert_eq!(before["apiKeyHelper"], after["apiKeyHelper"]);
    for key in ["ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        assert_eq!(before["env"][key], after["env"][key], "{key}");
    }
    assert_eq!(after["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "b/opus");
    assert_eq!(after["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"], "b/sonnet");
    // The profile's own alias is not part of a selection: it must not leak into the project.
    assert!(after["env"].get("ANTHROPIC_MODEL").is_none());
    // Re-applying with the stored selection is idempotent; dropping it returns to the profile's values.
    let again = binder.apply(&profile, &root, Some(&second.binding), false, second.binding.models.as_ref()).unwrap();
    assert!(!again.changed);
    assert_eq!(again.binding.models, second.binding.models);
    let plain = binder.apply(&profile, &root, Some(&again.binding), false, None).unwrap();
    assert!(plain.binding.models.is_none());
    assert_eq!(read(&local(&root))["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "team-opus");
    assert_eq!(binder.inspect(&plain.binding, &profile, &BTreeMap::new()).health, BindingHealth::Active);
    // A binding whose selection differs from what is on disk drifts.
    assert!(matches!(binder.inspect(&first.binding, &profile, &BTreeMap::new()).health, BindingHealth::Drifted(_)));
    assert!(binder.revert(&plain.binding).unwrap().is_empty());
    assert!(!Path::new(&local(&root)).exists());
}

#[test]
fn a_binding_without_selection_keeps_its_stored_form() {
    let json = serde_json::json!({"id": "b1", "path": "/x", "profileID": "p1", "managedValues": {}});
    let binding: WorkspaceBinding = serde_json::from_value(json.clone()).unwrap();
    assert!(binding.models.is_none());
    assert!(serde_json::to_value(&binding).unwrap().get("models").is_none());
}

#[test]
fn a_selection_is_stored_in_camel_case_and_without_empty_fields() {
    let mut binding = WorkspaceBinding::new("/x".into(), "p1".into());
    binding.models = Some(ModelSelection { tenant: "acme".into(), sonnet_model: Some("acme/claude-sonnet-5-5".into()), ..Default::default() });
    let value = serde_json::to_value(&binding).unwrap();
    assert_eq!(value["models"], json!({"tenant": "acme", "sonnetModel": "acme/claude-sonnet-5-5"}));
    assert_eq!(serde_json::from_value::<WorkspaceBinding>(value).unwrap(), binding);
}

fn office_client() -> OfficeClient {
    OfficeClient {
        enabled: true,
        addin_id: "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c".into(),
        gateway_url: "https://llm-gateway.example/api/x/desktop-gateway/office".into(),
        entra_client_id: "11111111-2222-4333-8444-555555555555".into(),
        entra_scope: "api://11111111-2222-4333-8444-555555555555/access_as_user".into(),
        disabled_features: vec!["web_search".into(), "thumbs".into()],
    }
}

#[test]
fn managed_profiles_read_the_office_block() {
    use keyzapper_core::managed::ManagedConfig;
    let values = serde_json::json!({"ManagedProfiles": [{
        "Name": "Company AI", "Endpoint": "https://llm-gateway.example/api/x/desktop-gateway/code", "Type": "oidc",
        "OIDCIssuer": "https://login.microsoftonline.com/t/v2.0", "OIDCClientID": "c", "OIDCTokenType": "id",
        "Office": {"AddinID": " 5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c ", "GatewayURL": "https://llm-gateway.example/api/x/desktop-gateway/office",
                   "EntraClientID": "11111111-2222-4333-8444-555555555555", "EntraScope": "api://11111111-2222-4333-8444-555555555555/access_as_user",
                   "DisabledFeatures": "web_search, thumbs"}
    }]});
    let config = ManagedConfig::from_values(values.as_object().unwrap());
    let office = config.profiles[0].office.clone().unwrap();
    assert!(office.enabled);
    assert_eq!(office.addin_id, "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c");
    assert_eq!(office.disabled_features, vec!["web_search", "thumbs"]);
    assert!(office.problems().is_empty());
}

#[test]
fn the_office_profile_prefers_managed_and_enabled_profiles() {
    use keyzapper_core::claude_office::office_profile;
    let mut own = sample();
    own.office = Some(OfficeClient { enabled: true, ..office_client() });
    let mut managed = other();
    managed.office = Some(OfficeClient { enabled: true, ..office_client() });
    let mut off = other();
    off.id = "33333333-0000-4000-8000-000000000003".into();
    off.office = Some(OfficeClient { enabled: false, ..office_client() });
    let profiles = vec![own.clone(), off, managed.clone()];
    assert_eq!(office_profile(&profiles, &[managed.id.clone()]).map(|p| p.id.as_str()), Some(managed.id.as_str()));
    assert_eq!(office_profile(&profiles, &[]).map(|p| p.id.as_str()), Some(own.id.as_str()));
    assert!(office_profile(&[], &[]).is_none());
}

#[test]
fn office_sync_touches_the_containers_only_on_change() {
    use keyzapper_core::claude_office::{plan_sync, DesiredOffice, OfficeApplied, SyncAction};
    let desired = DesiredOffice { profile_id: "p".into(), addin_id: "a".into(), fingerprint: "f1".into(), apps: vec!["excel".into()] };
    let applied = |id: &str, fp: &str| OfficeApplied { profile_id: "p".into(), addin_id: id.into(), fingerprint: fp.into(), apps: vec!["excel".into()] };
    assert_eq!(plan_sync(Some(&desired), None), SyncAction::Apply);
    assert_eq!(plan_sync(Some(&desired), Some(&applied("a", "f1"))), SyncAction::Nothing);
    assert_eq!(plan_sync(Some(&desired), Some(&applied("a", "f0"))), SyncAction::Apply);
    assert_eq!(plan_sync(Some(&desired), Some(&applied("old", "f1"))), SyncAction::Replace { old_addin_id: "old".into() });
    assert_eq!(plan_sync(None, Some(&applied("a", "f1"))), SyncAction::Remove { addin_id: "a".into() });
    assert_eq!(plan_sync(None, None), SyncAction::Nothing);
}

#[test]
fn another_app_list_or_a_repeated_request_registers_again() {
    use keyzapper_core::claude_office::{plan_retry, plan_sync, DesiredOffice, OfficeApplied, SyncAction};
    let desired = DesiredOffice { profile_id: "p".into(), addin_id: "a".into(), fingerprint: "f1".into(), apps: vec!["excel".into(), "word".into()] };
    let applied = OfficeApplied { profile_id: "p".into(), addin_id: "a".into(), fingerprint: "f1".into(), apps: vec!["excel".into()] };
    // Office got installed (or removed) since: same manifest, other apps.
    assert_eq!(plan_sync(Some(&desired), Some(&applied)), SyncAction::Apply);
    // A user-initiated run repeats what failed although nothing changed; everything else plans as before.
    let same = OfficeApplied { apps: desired.apps.clone(), ..applied.clone() };
    assert_eq!(plan_sync(Some(&desired), Some(&same)), SyncAction::Nothing);
    assert_eq!(plan_retry(Some(&desired), Some(&same)), SyncAction::Apply);
    assert_eq!(plan_retry(Some(&desired), None), SyncAction::Apply);
    assert_eq!(plan_retry(Some(&desired), Some(&OfficeApplied { addin_id: "old".into(), ..same.clone() })), SyncAction::Replace { old_addin_id: "old".into() });
    assert_eq!(plan_retry(None, Some(&same)), SyncAction::Remove { addin_id: "a".into() });
    assert_eq!(plan_retry(None, None), SyncAction::Nothing);
}

#[test]
fn the_office_block_of_a_managed_profile_is_optional_and_a_broken_one_is_kept() {
    use keyzapper_core::managed::{ManagedConfig, KEYS};
    let profile = |office: Value| {
        let values = json!({"ManagedProfiles": [{"Name": "P", "Endpoint": "https://llm-gateway.example/api/x/desktop-gateway/code", "Office": office}]});
        ManagedConfig::from_values(values.as_object().unwrap()).profiles.remove(0).office
    };
    let without = json!({"ManagedProfiles": [{"Name": "P", "Endpoint": "https://llm-gateway.example/x"}]});
    assert!(ManagedConfig::from_values(without.as_object().unwrap()).profiles[0].office.is_none());
    assert!(profile(json!("not a dict")).is_none());
    // `Enabled` defaults to on; strings and numbers count like the other switches.
    assert!(profile(json!({})).unwrap().enabled);
    assert!(!profile(json!({"Enabled": false})).unwrap().enabled);
    assert!(!profile(json!({"Enabled": "0"})).unwrap().enabled);
    // A block with missing values stays; the sync reports what is wrong with it.
    let broken = profile(json!({"AddinID": "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c"})).unwrap();
    assert_eq!(broken.gateway_url, "");
    assert!(!broken.problems().is_empty());
    // Disabled features: an array or a comma separated string, trimmed, without empty entries.
    assert_eq!(profile(json!({"DisabledFeatures": ["web_search", " thumbs ", ""]})).unwrap().disabled_features, vec!["web_search", "thumbs"]);
    assert_eq!(profile(json!({"DisabledFeatures": " web_search,, thumbs "})).unwrap().disabled_features, vec!["web_search", "thumbs"]);
    assert!(profile(json!({"DisabledFeatures": ""})).unwrap().disabled_features.is_empty());
    // The block lives inside the profile dict; no extra top-level setting is read.
    assert!(!KEYS.contains(&"Office"));
}

#[test]
fn the_claude_desktop_block_of_a_managed_profile_is_optional_and_a_broken_one_is_kept() {
    use keyzapper_core::claude_desktop::{BearerTokenType, CredentialKind};
    use keyzapper_core::managed::ManagedConfig;
    let parse = |block: Value| {
        let values = json!({"ManagedProfiles": [{"Name": "P", "Endpoint": "https://llm-gateway.example/code", "ClaudeDesktop": block}]});
        ManagedConfig::from_values(values.as_object().unwrap()).profiles.remove(0).claude_desktop
    };
    assert!(parse(json!("not a dict")).is_none());
    let sso = parse(json!({
        "GatewayURL": " https://llm-gateway.example/desktop ",
        "OIDCIssuer": "https://login.example.test/t/v2.0",
        "OIDCClientID": "c1",
        "BearerTokenType": "access_token",
        "Scopes": "api://gateway/.default",
    }))
    .unwrap();
    assert_eq!((sso.gateway_url.as_str(), sso.bearer_token_type), ("https://llm-gateway.example/desktop", BearerTokenType::AccessToken));
    assert!(sso.problems().is_empty());
    assert_eq!(sso.credential_kind, CredentialKind::ExternalIdp);
    // `id_token` is the default; a block with missing values stays and reports what is wrong.
    let broken = parse(json!({})).unwrap();
    assert_eq!(broken.bearer_token_type, BearerTokenType::IdToken);
    assert_eq!(broken.credential_kind, CredentialKind::ExternalIdp);
    assert!(!broken.problems().is_empty());
    // `external-idp` is the default kind; an unknown one keeps the block and is reported.
    let with_kind = |kind: &str| parse(json!({"OIDCIssuer": "https://login.example.test/t/v2.0", "OIDCClientID": "c1", "CredentialKind": kind})).unwrap();
    assert_eq!(with_kind("").credential_kind, CredentialKind::ExternalIdp);
    assert_eq!(with_kind("external-idp").credential_kind, CredentialKind::ExternalIdp);
    assert_eq!(with_kind(" Interactive ").credential_kind, CredentialKind::Interactive);
    assert!(with_kind("interactive").problems().is_empty() && with_kind("external-idp").problems().is_empty());
    let bogus = with_kind("bogus");
    assert_eq!(bogus.credential_kind, CredentialKind::Unknown);
    assert_eq!(bogus.problems().len(), 1);
    // Further keys are taken as they are; `AutoApply` is off unless set.
    assert!(!sso.auto_apply && sso.config.is_empty());
    let auto = parse(json!({"OIDCIssuer": "https://login.example.test/t/v2.0", "OIDCClientID": "c1", "AutoApply": true,
        "Config": {"disableDeploymentModeChooser": true, "coworkEgressAllowedHosts": ["example.com"]}}))
    .unwrap();
    assert!(auto.auto_apply && auto.problems().is_empty());
    assert_eq!(Value::Object(auto.config), json!({"disableDeploymentModeChooser": true, "coworkEgressAllowedHosts": ["example.com"]}));
    // The installation without a click is a separate switch, off by default.
    assert!(!ManagedConfig::from_values(&serde_json::Map::new()).claude_desktop_auto_install);
    assert!(ManagedConfig::from_values(json!({"ClaudeDesktopAutoInstall": true}).as_object().unwrap()).claude_desktop_auto_install);
}

#[test]
fn office_settings_are_stored_only_when_present() {
    use keyzapper_core::claude_office::OfficeApplied;
    let plain = serde_json::to_value(sample()).unwrap();
    assert!(plain.get("office").is_none());
    let mut profile = sample();
    profile.office = Some(office_client());
    let stored = serde_json::to_value(&profile).unwrap();
    assert_eq!(stored["office"]["addinId"], "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c");
    assert_eq!(stored["office"]["disabledFeatures"], json!(["web_search", "thumbs"]));
    assert_eq!(serde_json::from_value::<Profile>(stored).unwrap(), profile);
    // Files written before Office existed still load.
    assert!(serde_json::from_value::<Profile>(plain).unwrap().office.is_none());

    let empty = serde_json::to_value(AppState::default()).unwrap();
    assert!(empty.get("officeApplied").is_none() && empty.get("officeError").is_none());
    let state = AppState {
        office_applied: Some(OfficeApplied { profile_id: ALPHA.into(), addin_id: "a".into(), fingerprint: "f".into(), apps: vec!["excel".into()] }),
        office_error: Some("no access".into()),
        ..AppState::default()
    };
    let stored = serde_json::to_value(&state).unwrap();
    assert_eq!(stored["officeApplied"], json!({"profileId": ALPHA, "addinId": "a", "fingerprint": "f", "apps": ["excel"]}));
    assert_eq!(serde_json::from_value::<AppState>(stored).unwrap(), state);
}

mod claude_office {
    use keyzapper_core::claude_office::{apply_mac, fingerprint, installed_apps, manifest, parameters, remove_mac, OfficeApp, OfficeClient, MARKETPLACE_ADDIN_ID};

    fn office() -> OfficeClient {
        OfficeClient {
            enabled: true,
            addin_id: "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c".into(),
            gateway_url: "https://llm-gateway.example/api/x/desktop-gateway/office".into(),
            entra_client_id: "11111111-2222-4333-8444-555555555555".into(),
            entra_scope: "api://11111111-2222-4333-8444-555555555555/access_as_user".into(),
            disabled_features: vec!["web_search".into(), "thumbs".into()],
        }
    }

    #[test]
    fn manifest_carries_the_id_and_the_entra_parameters_in_both_url_slots() {
        let template = include_str!("fixtures/office-manifest.xml");
        let xml = manifest(template, &office()).unwrap();
        assert!(xml.contains("<Id>5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c</Id>") && !xml.contains(MARKETPLACE_ADDIN_ID));
        let url = "https://pivot.claude.ai/?m=unified-1.0.0.12&amp;gateway_url=https%3A%2F%2Fllm-gateway.example%2Fapi%2Fx%2Fdesktop-gateway%2Foffice&amp;gateway_api_format=anthropic&amp;entra_sso=1&amp;graph_client_id=11111111-2222-4333-8444-555555555555&amp;entra_scope=api%3A%2F%2F11111111-2222-4333-8444-555555555555%2Faccess_as_user&amp;gateway_auth_source=entra&amp;disabled_features=web_search%2Cthumbs";
        assert_eq!(xml.matches(&format!("DefaultValue=\"{url}\"")).count(), 2);
        assert!(!xml.contains("gateway_token"));
    }

    #[test]
    fn manifest_fails_closed_on_unknown_templates_and_bad_settings() {
        let template = include_str!("fixtures/office-manifest.xml");
        assert!(manifest(&template.replace("<SourceLocation", "<Elsewhere"), &office()).is_err());
        assert!(manifest(&template.replace("?m=unified-1.0.0.12\" />", "?m=unified-1.0.0.12#x\" />"), &office()).is_err());
        assert!(manifest(&template.replace("id=\"Taskpane.Url\" DefaultValue=\"https://pivot.claude.ai/?m=unified-1.0.0.12\"", "id=\"Taskpane.Url\" DefaultValue=\"https://pivot.claude.ai/other\""), &office()).is_err());
        let mut marketplace = office();
        marketplace.addin_id = MARKETPLACE_ADDIN_ID.into();
        assert!(manifest(template, &marketplace).is_err());
        let mut local = office();
        local.gateway_url = "https://localhost/office".into();
        assert!(!local.problems().is_empty());
    }

    #[test]
    fn a_template_without_an_id_yields_no_manifest() {
        let template = include_str!("fixtures/office-manifest.xml");
        assert!(manifest(template, &office()).is_ok());
        let without = |from: &str, to: &str| manifest(&template.replace(from, to), &office());
        assert!(without("<Id>4e2fd29d-dad0-40b3-af3c-a16e347a5ddc</Id>", "").is_err(), "no <Id> element");
        assert!(without("<Id>", "<Ident>").is_err(), "no <Id> start tag");
        assert!(without("</Id>", "</Ident>").is_err(), "an <Id> that is never closed");
    }

    #[test]
    fn the_parameters_follow_the_base_url_of_the_template_with_the_right_separator() {
        let template = include_str!("fixtures/office-manifest.xml");
        let parameters = "gateway_url=https%3A%2F%2Fllm-gateway.example%2Fapi%2Fx%2Fdesktop-gateway%2Foffice&amp;gateway_api_format=anthropic&amp;entra_sso=1&amp;graph_client_id=11111111-2222-4333-8444-555555555555&amp;entra_scope=api%3A%2F%2F11111111-2222-4333-8444-555555555555%2Faccess_as_user&amp;gateway_auth_source=entra&amp;disabled_features=web_search%2Cthumbs";
        let with_base = |base: &str| manifest(&template.replace("https://pivot.claude.ai/?m=unified-1.0.0.12", base), &office()).unwrap();
        // The base already has a query: the parameters are appended with `&` (escaped as `&amp;` in the XML), not a second `?`.
        let xml = with_base("https://pivot.claude.ai/?m=unified-1.0.0.12&amp;x=1");
        assert_eq!(xml.matches(&format!("DefaultValue=\"https://pivot.claude.ai/?m=unified-1.0.0.12&amp;x=1&amp;{parameters}\"")).count(), 2);
        assert!(!xml.contains("?m=unified-1.0.0.12&amp;x=1?"));
        // A base without a query gets the first parameter after `?`.
        let xml = with_base("https://pivot.claude.ai/taskpane");
        assert_eq!(xml.matches(&format!("DefaultValue=\"https://pivot.claude.ai/taskpane?{parameters}\"")).count(), 2);
    }

    #[test]
    fn sideload_on_mac_writes_only_its_own_files_and_removes_them() {
        let root = std::env::temp_dir().join(format!("kz-office-{}", uuid::Uuid::new_v4()));
        let (containers, applications) = (root.join("Containers"), root.join("Applications"));
        std::fs::create_dir_all(applications.join("Microsoft Excel.app")).unwrap();
        std::fs::create_dir_all(applications.join("Microsoft Word.app")).unwrap();
        let wef = |bundle: &str| containers.join(bundle).join("Data/Documents/wef");
        std::fs::create_dir_all(wef("com.microsoft.Excel")).unwrap();
        std::fs::write(wef("com.microsoft.Excel").join("other.manifest.xml"), "x").unwrap();
        let apps = installed_apps(&applications);
        assert_eq!(apps, vec![OfficeApp::Excel, OfficeApp::Word]);
        let id = "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c";
        apply_mac(&containers, &apps, id, "<xml/>").unwrap();
        apply_mac(&containers, &apps, id, "<xml/>").unwrap();
        assert_eq!(std::fs::read_to_string(wef("com.microsoft.Word").join(format!("{id}.manifest.xml"))).unwrap(), "<xml/>");
        assert!(!wef("com.microsoft.Powerpoint").exists());
        remove_mac(&containers, id).unwrap();
        assert!(!wef("com.microsoft.Excel").join(format!("{id}.manifest.xml")).exists());
        assert!(wef("com.microsoft.Excel").join("other.manifest.xml").exists());
        remove_mac(&containers, id).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_unusable_setting_is_reported_once() {
        assert!(office().problems().is_empty());
        let broken = |change: fn(&mut OfficeClient)| {
            let mut client = office();
            change(&mut client);
            client.problems().len()
        };
        assert_eq!(broken(|o| o.addin_id = o.addin_id.to_uppercase()), 1);
        assert_eq!(broken(|o| o.gateway_url = "http://llm-gateway.example/office".into()), 1);
        assert_eq!(broken(|o| o.gateway_url = "https://127.0.0.1/office".into()), 1);
        assert_eq!(broken(|o| o.gateway_url = "https://[::1]/office".into()), 1);
        assert_eq!(broken(|o| o.gateway_url = "https://user:secret@llm-gateway.example/office".into()), 1);
        assert_eq!(broken(|o| o.entra_client_id = "not-a-guid".into()), 1);
        assert_eq!(broken(|o| o.entra_scope = "api://x/access as user".into()), 1);
        assert_eq!(broken(|o| o.disabled_features.push("Web Search".into())), 1);
    }

    #[test]
    fn parameters_keep_their_order_and_leave_out_empty_disabled_features() {
        let mut client = office();
        client.disabled_features.clear();
        let names: Vec<_> = parameters(&client).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["gateway_url", "gateway_api_format", "entra_sso", "graph_client_id", "entra_scope", "gateway_auth_source"]);
    }

    #[test]
    fn the_fingerprint_is_the_sha256_hex_and_an_id_with_path_syntax_is_refused() {
        assert_eq!(fingerprint("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let dir = std::env::temp_dir().join(format!("kz-office-{}", uuid::Uuid::new_v4()));
        assert!(apply_mac(&dir, &[OfficeApp::Word], "../x", "<xml/>").is_err());
        assert!(remove_mac(&dir, "a/b").is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn ids_that_are_not_a_company_guid_are_refused_without_touching_anything() {
        let root = std::env::temp_dir().join(format!("kz-office-{}", uuid::Uuid::new_v4()));
        let wef = root.join("com.microsoft.Excel/Data/Documents/wef");
        std::fs::create_dir_all(&wef).unwrap();
        let upper = "5C1D7E2A-8B3F-4E6D-9A0C-2F4B6D8E1A3C";
        for id in ["other", MARKETPLACE_ADDIN_ID, upper, "", "5c1d7e2a8b3f4e6d9a0c2f4b6d8e1a3c"] {
            std::fs::write(wef.join(format!("{id}.manifest.xml")), "foreign").unwrap();
            assert!(apply_mac(&root, &[OfficeApp::Excel, OfficeApp::Word], id, "<xml/>").is_err(), "apply accepted {id:?}");
            assert!(remove_mac(&root, id).is_err(), "remove accepted {id:?}");
            assert_eq!(std::fs::read_to_string(wef.join(format!("{id}.manifest.xml"))).unwrap(), "foreign");
        }
        assert!(!root.join("com.microsoft.Word").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn one_unwritable_app_does_not_stop_the_others() {
        let root = std::env::temp_dir().join(format!("kz-office-{}", uuid::Uuid::new_v4()));
        let id = "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c";
        let wef = |bundle: &str| root.join(bundle).join("Data/Documents/wef");
        // A file where Excel's wef folder should be: Excel can neither be written nor cleaned.
        std::fs::create_dir_all(wef("com.microsoft.Excel").parent().unwrap()).unwrap();
        std::fs::write(wef("com.microsoft.Excel"), "not a folder").unwrap();
        let apps = [OfficeApp::Excel, OfficeApp::Word, OfficeApp::PowerPoint];
        assert!(apply_mac(&root, &apps, id, "<xml/>").is_err());
        for bundle in ["com.microsoft.Word", "com.microsoft.Powerpoint"] {
            assert_eq!(std::fs::read_to_string(wef(bundle).join(format!("{id}.manifest.xml"))).unwrap(), "<xml/>");
        }
        assert!(remove_mac(&root, id).is_err());
        for bundle in ["com.microsoft.Word", "com.microsoft.Powerpoint"] {
            assert!(!wef(bundle).join(format!("{id}.manifest.xml")).exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn sideload_on_windows_registers_and_unregisters_only_its_value() {
        use keyzapper_core::claude_office::{apply_windows, remove_windows};
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        use winreg::RegKey;
        let key_path = format!(r"SOFTWARE\KeyZapperTest\{}", uuid::Uuid::new_v4());
        let dir = std::env::temp_dir().join(format!("kz-office-{}", uuid::Uuid::new_v4()));
        let id = "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c";
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu.create_subkey(&key_path).unwrap();
        key.set_value("other", &"C:\\other.manifest.xml").unwrap();
        apply_windows(&dir, &key_path, id, "<xml/>").unwrap();
        apply_windows(&dir, &key_path, id, "<xml/>").unwrap();
        let file = dir.join(format!("{id}.manifest.xml"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "<xml/>");
        let registered: String = key.get_value(id).unwrap();
        assert_eq!(std::path::PathBuf::from(&registered), std::path::absolute(&file).unwrap());
        remove_windows(&dir, &key_path, id).unwrap();
        remove_windows(&dir, &key_path, id).unwrap();
        assert!(!file.exists());
        let names: Vec<String> = hkcu.open_subkey_with_flags(&key_path, KEY_READ).unwrap().enum_values().map(|v| v.unwrap().0).collect();
        assert_eq!(names, vec!["other".to_string()]);
        let _ = hkcu.delete_subkey_all(&key_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

mod office_sync {
    use super::{office_client, one_shot_server};
    use keyzapper_core::claude_office::{fetch_template_from, manifest_from, status, DesiredOffice, OfficeApp, OfficeApplied, OfficeState};
    use std::path::PathBuf;
    use std::time::Duration;

    const TEMPLATE: &str = include_str!("fixtures/office-manifest.xml");
    const ID: &str = "5c1d7e2a-8b3f-4e6d-9a0c-2f4b6d8e1a3c";

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("kz-office-sync-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn the_template_is_fetched_from_the_given_address_and_anything_but_200_fails() {
        let (endpoint, server) = one_shot_server("200 OK", TEMPLATE);
        assert_eq!(fetch_template_from(&format!("{endpoint}/manifest.xml"), Duration::from_secs(5)).unwrap(), TEMPLATE);
        assert!(server.join().unwrap().to_lowercase().starts_with("get /manifest.xml http/1.1"));
        let (endpoint, server) = one_shot_server("404 Not Found", "nope");
        let error = fetch_template_from(&endpoint, Duration::from_secs(5)).unwrap_err();
        server.join().unwrap();
        assert!(error.contains("404"), "{error}");
        // Nothing listens there.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        assert!(fetch_template_from(&format!("http://{address}"), Duration::from_secs(5)).is_err());
    }

    #[test]
    fn a_fetched_template_is_kept_and_used_when_pivot_cannot_be_reached() {
        let dir = scratch();
        let file = dir.join("office-manifest-template.xml");
        let office = office_client();
        // Without a stored template and without a fetched one there is nothing to build from, and nothing is stored.
        let error = manifest_from(&file, Some(Err("timed out".into())), &office).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(manifest_from(&file, None, &office).is_err());
        assert!(!file.exists());
        // A good fetch is used and stored.
        let fresh = manifest_from(&file, Some(Ok(TEMPLATE.into())), &office).unwrap();
        assert!(fresh.contains(&format!("<Id>{ID}</Id>")));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), TEMPLATE);
        // Later, a failed fetch (or none) falls back to the stored template and gives the same manifest.
        assert_eq!(manifest_from(&file, Some(Err("offline".into())), &office).unwrap(), fresh);
        assert_eq!(manifest_from(&file, None, &office).unwrap(), fresh);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fetched_template_of_an_unknown_shape_is_neither_used_nor_stored() {
        let dir = scratch();
        let file = dir.join("office-manifest-template.xml");
        let office = office_client();
        manifest_from(&file, Some(Ok(TEMPLATE.into())), &office).unwrap();
        // 200 with a login page of a proxy, or a template without the URL slots: fail, the good copy stays.
        assert!(manifest_from(&file, Some(Ok("<html>login</html>".into())), &office).is_err());
        assert!(manifest_from(&file, Some(Ok(TEMPLATE.replace("<SourceLocation", "<Elsewhere"))), &office).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), TEMPLATE);
        // Unusable settings fail before anything is stored.
        let mut marketplace = office_client();
        marketplace.addin_id = keyzapper_core::claude_office::MARKETPLACE_ADDIN_ID.into();
        let other = dir.join("other.xml");
        assert!(manifest_from(&other, Some(Ok(TEMPLATE.into())), &marketplace).is_err());
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_desired_state_names_the_apps_and_fingerprints_the_manifest() {
        let desired = DesiredOffice::new("p", &office_client(), "<xml/>", &[OfficeApp::Excel, OfficeApp::PowerPoint]);
        assert_eq!(desired.profile_id, "p");
        assert_eq!(desired.addin_id, ID);
        assert_eq!(desired.fingerprint, keyzapper_core::claude_office::fingerprint("<xml/>"));
        assert_eq!(desired.apps, vec!["excel", "powerpoint"]);
    }

    #[test]
    fn the_status_tells_what_is_registered_and_what_went_wrong() {
        let applied = OfficeApplied { profile_id: "p".into(), addin_id: ID.into(), fingerprint: "f".into(), apps: vec!["excel".into(), "word".into()] };
        let off = status(None, None);
        assert_eq!((off.state, off.apps.is_empty(), off.message), (OfficeState::Off, true, None));
        let ok = status(Some(&applied), None);
        assert_eq!((ok.state, ok.apps.clone(), ok.message.clone()), (OfficeState::Applied, vec!["excel".to_string(), "word".to_string()], None));
        // An error keeps showing what is still registered.
        let failed = status(Some(&applied), Some("denied"));
        assert_eq!((failed.state, failed.apps.len(), failed.message.as_deref()), (OfficeState::Error, 2, Some("denied")));
        let blocked = status(None, Some("bad id"));
        assert_eq!((blocked.state, blocked.apps.is_empty(), blocked.message.as_deref()), (OfficeState::Error, true, Some("bad id")));
        assert_eq!(serde_json::to_value(&ok).unwrap(), serde_json::json!({"state": "applied", "apps": ["excel", "word"], "message": null}));
    }

    /// Running a job writes into the folders of the apps; Windows registers in the registry instead (own test in `claude_office`).
    #[cfg(not(windows))]
    mod run {
        use super::*;
        use keyzapper_core::claude_office::{Sideload, SyncAction, SyncJob};
        use std::path::Path;

        const OLD_ID: &str = "0a0a0a0a-1111-4222-8333-444444444444";

        fn sideload(root: &Path) -> Sideload {
            Sideload {
                containers_dir: root.join("Containers"),
                applications_dir: root.join("Applications"),
                manifest_dir: root.join("office"),
                registry_key: r"SOFTWARE\KeyZapperTest".into(),
            }
        }

        fn wef(root: &Path, bundle: &str) -> PathBuf {
            root.join("Containers").join(bundle).join("Data/Documents/wef")
        }

        fn manifest_of(root: &Path, bundle: &str, id: &str) -> PathBuf {
            wef(root, bundle).join(format!("{id}.manifest.xml"))
        }

        const ALL: [OfficeApp; 3] = [OfficeApp::Excel, OfficeApp::Word, OfficeApp::PowerPoint];

        fn desired(id: &str, apps: &[OfficeApp]) -> DesiredOffice {
            let mut office = office_client();
            office.addin_id = id.into();
            DesiredOffice::new("p", &office, "<xml/>", apps)
        }

        fn applied(id: &str, apps: &[OfficeApp]) -> OfficeApplied {
            let d = desired(id, apps);
            OfficeApplied { profile_id: d.profile_id, addin_id: d.addin_id, fingerprint: d.fingerprint, apps: d.apps }
        }

        fn job(action: SyncAction, desired: Option<DesiredOffice>, apps: &[OfficeApp], previous: Option<OfficeApplied>) -> SyncJob {
            SyncJob { action, desired, manifest: "<xml/>".into(), apps: apps.to_vec(), previous }
        }

        #[test]
        fn apply_writes_the_apps_it_is_given_and_records_them() {
            let root = scratch();
            let apps = [OfficeApp::Excel, OfficeApp::Word];
            let outcome = job(SyncAction::Apply, Some(desired(ID, &apps)), &apps, None).run(&sideload(&root), &mut |_| true);
            assert_eq!((outcome.applied.clone(), outcome.error), (Some(applied(ID, &apps)), None));
            assert_eq!(std::fs::read_to_string(manifest_of(&root, "com.microsoft.Word", ID)).unwrap(), "<xml/>");
            assert!(!wef(&root, "com.microsoft.Powerpoint").exists());
            // No Office installed: nothing to write, yet the state is recorded, so the sync notices a later installation.
            let none = job(SyncAction::Apply, Some(desired(OLD_ID, &[])), &[], None).run(&sideload(&root), &mut |_| true);
            assert_eq!(none.applied.unwrap().apps, Vec::<String>::new());
            assert!(none.error.is_none() && !manifest_of(&root, "com.microsoft.Excel", OLD_ID).exists());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn a_partly_failed_apply_is_recorded_so_that_removing_cleans_everything() {
            let root = scratch();
            // A file where Excel's wef folder should be: Excel cannot be written, Word and PowerPoint can.
            std::fs::create_dir_all(wef(&root, "com.microsoft.Excel").parent().unwrap()).unwrap();
            std::fs::write(wef(&root, "com.microsoft.Excel"), "not a folder").unwrap();
            let outcome = job(SyncAction::Apply, Some(desired(ID, &ALL)), &ALL, None).run(&sideload(&root), &mut |_| true);
            assert_eq!(outcome.applied, Some(applied(ID, &ALL)), "the intended apps and fingerprint are saved");
            assert!(outcome.error.is_some());
            for bundle in ["com.microsoft.Word", "com.microsoft.Powerpoint"] {
                assert!(manifest_of(&root, bundle, ID).exists());
            }
            // Later the problem is gone and the user deactivates KeyZapper: the recorded state finds everything.
            std::fs::remove_file(wef(&root, "com.microsoft.Excel")).unwrap();
            let removal = job(SyncAction::Remove { addin_id: ID.into() }, None, &[], outcome.applied).run(&sideload(&root), &mut |_| true);
            assert_eq!((removal.applied, removal.error), (None, None));
            for bundle in ["com.microsoft.Word", "com.microsoft.Powerpoint"] {
                assert!(!manifest_of(&root, bundle, ID).exists());
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn removing_keeps_the_record_while_a_file_cannot_be_removed() {
            let root = scratch();
            let apps = [OfficeApp::Word];
            let sideload = sideload(&root);
            job(SyncAction::Apply, Some(desired(ID, &apps)), &apps, None).run(&sideload, &mut |_| true);
            std::fs::create_dir_all(wef(&root, "com.microsoft.Excel").parent().unwrap()).unwrap();
            std::fs::write(wef(&root, "com.microsoft.Excel"), "not a folder").unwrap();
            let previous = applied(ID, &apps);
            let outcome = job(SyncAction::Remove { addin_id: ID.into() }, None, &[], Some(previous.clone())).run(&sideload, &mut |_| true);
            assert_eq!(outcome.applied, Some(previous), "so that the next run tries again");
            assert!(outcome.error.is_some());
            assert!(!manifest_of(&root, "com.microsoft.Word", ID).exists(), "the other apps are cleaned anyway");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn replacing_removes_the_old_manifest_first_and_writes_the_new_one() {
            let root = scratch();
            let apps = [OfficeApp::Excel, OfficeApp::Word];
            let sideload = sideload(&root);
            job(SyncAction::Apply, Some(desired(OLD_ID, &apps)), &apps, None).run(&sideload, &mut |_| true);
            std::fs::write(wef(&root, "com.microsoft.Excel").join("foreign.manifest.xml"), "x").unwrap();
            let replace = SyncAction::Replace { old_addin_id: OLD_ID.into() };
            let outcome = job(replace, Some(desired(ID, &apps)), &apps, Some(applied(OLD_ID, &apps))).run(&sideload, &mut |_| true);
            assert_eq!((outcome.applied, outcome.error), (Some(applied(ID, &apps)), None));
            for bundle in ["com.microsoft.Excel", "com.microsoft.Word"] {
                assert!(!manifest_of(&root, bundle, OLD_ID).exists() && manifest_of(&root, bundle, ID).exists());
            }
            assert!(wef(&root, "com.microsoft.Excel").join("foreign.manifest.xml").exists());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn replacing_stops_before_writing_when_the_old_manifest_stays() {
            let root = scratch();
            let sideload = sideload(&root);
            job(SyncAction::Apply, Some(desired(OLD_ID, &[OfficeApp::Word])), &[OfficeApp::Word], None).run(&sideload, &mut |_| true);
            std::fs::create_dir_all(wef(&root, "com.microsoft.Excel").parent().unwrap()).unwrap();
            std::fs::write(wef(&root, "com.microsoft.Excel"), "not a folder").unwrap();
            let previous = applied(OLD_ID, &[OfficeApp::Word]);
            let replace = SyncAction::Replace { old_addin_id: OLD_ID.into() };
            let outcome = job(replace, Some(desired(ID, &ALL)), &ALL, Some(previous.clone())).run(&sideload, &mut |_| true);
            // The old record stays, so nothing ends up in a folder KeyZapper no longer knows about.
            assert_eq!(outcome.applied, Some(previous));
            assert!(outcome.error.is_some());
            for bundle in ["com.microsoft.Word", "com.microsoft.Powerpoint"] {
                assert!(!manifest_of(&root, bundle, ID).exists());
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn the_intended_record_is_saved_before_anything_is_written() {
            let root = scratch();
            let sideload = sideload(&root);
            let apps = [OfficeApp::Excel, OfficeApp::Word];
            let mut seen = vec![];
            let outcome = job(SyncAction::Apply, Some(desired(ID, &apps)), &apps, None).run(&sideload, &mut |interim| {
                // A crash right here finds the record, and no file yet.
                seen.push((interim.clone(), manifest_of(&root, "com.microsoft.Excel", ID).exists() || manifest_of(&root, "com.microsoft.Word", ID).exists()));
                true
            });
            assert_eq!(seen.len(), 1);
            let (interim, written) = &seen[0];
            assert_eq!(interim.applied, Some(applied(ID, &apps)));
            assert!(interim.error.is_some() && !written, "the run is marked unfinished until it is done");
            assert_eq!((outcome.applied, outcome.error), (Some(applied(ID, &apps)), None));
            assert!(manifest_of(&root, "com.microsoft.Excel", ID).exists());

            // A replacement removes the old manifest first and records the new one only afterwards.
            let replace = SyncAction::Replace { old_addin_id: ID.into() };
            let mut seen = vec![];
            let outcome = job(replace, Some(desired(OLD_ID, &apps)), &apps, Some(applied(ID, &apps))).run(&sideload, &mut |interim| {
                seen.push((interim.applied.clone(), manifest_of(&root, "com.microsoft.Excel", ID).exists(), manifest_of(&root, "com.microsoft.Excel", OLD_ID).exists()));
                true
            });
            assert_eq!(seen, vec![(Some(applied(OLD_ID, &apps)), false, false)]);
            assert_eq!(outcome.applied, Some(applied(OLD_ID, &apps)));
            assert!(manifest_of(&root, "com.microsoft.Excel", OLD_ID).exists());

            // A removal has nothing to record beforehand.
            let mut calls = 0;
            job(SyncAction::Remove { addin_id: OLD_ID.into() }, None, &[], Some(applied(OLD_ID, &apps))).run(&sideload, &mut |_| {
                calls += 1;
                true
            });
            assert_eq!(calls, 0);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn nothing_is_written_when_the_record_cannot_be_saved() {
            let root = scratch();
            let apps = [OfficeApp::Word];
            let previous = applied(OLD_ID, &apps);
            let outcome = job(SyncAction::Apply, Some(desired(ID, &apps)), &apps, Some(previous.clone())).run(&sideload(&root), &mut |_| false);
            assert_eq!(outcome.applied, Some(previous));
            assert!(outcome.error.is_some());
            assert!(!root.exists(), "no folder was touched");
        }

        #[test]
        fn a_run_without_a_change_touches_nothing() {
            let root = scratch();
            let apps = [OfficeApp::Word];
            let outcome = job(SyncAction::Nothing, Some(desired(ID, &apps)), &apps, Some(applied(ID, &apps))).run(&sideload(&root), &mut |_| true);
            assert_eq!(outcome.applied, Some(applied(ID, &apps)));
            assert!(outcome.error.is_none() && !root.exists());
        }
    }
}
