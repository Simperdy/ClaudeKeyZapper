//! Private synthetic config transport tests; these do not establish build support.
use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!("hermes-cli-read-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&path).unwrap(); path
}
fn fake_cli(root: &Path, script: &str) -> HermesTarget {
    let exe = root.join("fake hermes; $(touch pwned)'");
    fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    HermesTarget { executable: exe, home: root.join("selected home; $(touch home-pwned)'"), display_name: "test".into() }
}
fn synthetic_cli(target: &HermesTarget) -> HermesCli {
    HermesCli::with_evidence(CompatibilityEvidence::validated(target, true))
}

#[test]
fn cli_inherited_profile_harness() {
    if std::env::var_os("HERMES_PROFILE_HARNESS").is_none() { return; }
    let root = temp();
    let target = fake_cli(&root, "[ -z \"${ANTHROPIC_API_KEY+x}\" ] || exit 9\nprintf '%s\\n' \"$1 $2|$HERMES_HOME|${HERMES_PROFILE-unset}\" >> \"$HERMES_HOME/calls\"\nif [ \"$1 $2\" = 'config path' ]; then printf '%s\\n' \"$HERMES_HOME/config.yaml\"; fi");
    fs::create_dir_all(&target.home).unwrap();
    synthetic_cli(&target).snapshot(&target, &[]).unwrap();
    let calls = fs::read_to_string(target.home.join("calls")).unwrap();
    assert!(calls.contains("config path|")); assert!(calls.contains("|unset"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_removes_an_inherited_profile_in_an_isolated_child_harness() {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "hermes::cli::read_tests::cli_inherited_profile_harness", "--nocapture"])
        .env("HERMES_PROFILE", "inherited-sensitive-profile").env("HERMES_PROFILE_HARNESS", "1")
        .env("ANTHROPIC_API_KEY", "synthetic-inherited-env-must-not-reach-cli").status().unwrap();
    assert!(status.success());
}
#[test]
fn cli_snapshot_preserves_absence_reads_raw_yaml_and_uses_scoped_argv() {
    let root = temp();
    let target = fake_cli(&root, "printf '%s\\n' \"$*|$HERMES_HOME|${HERMES_PROFILE-unset}\" >> \"$HERMES_HOME/calls\"\nif [ \"$1 $2\" = 'config path' ]; then printf '%s\\n' \"$HERMES_HOME/config.yaml\"; fi");
    fs::create_dir_all(&target.home).unwrap();
    fs::write(target.home.join("config.yaml"), "model:\n  default: claude-3\nprovider:\n  unrelated: secret-token-value\n").unwrap();
    let cli = synthetic_cli(&target);
    let snapshot = cli.snapshot(&target, &["model.default".into(), "model.base_url".into()]).unwrap();
    assert_eq!(cli.raw_config(&target).unwrap().unwrap(), fs::read(target.home.join("config.yaml")).unwrap());
    assert!(matches!(snapshot.fields["model.default"], StoredValue::Present(ref v) if v == &json!("claude-3")));
    assert!(matches!(snapshot.fields["model.base_url"], StoredValue::Absent));
    let calls = fs::read_to_string(target.home.join("calls")).unwrap();
    assert!(calls.contains("config path|")); assert!(calls.contains("|unset")); assert!(!root.join("pwned").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_snapshot_rejects_malformed_yaml_without_leaking_values() {
    let root = temp(); let target = fake_cli(&root, "printf '%s\\n' \"$HERMES_HOME/config.yaml\"");
    fs::create_dir_all(&target.home).unwrap(); fs::write(target.home.join("config.yaml"), "bad: [secret-token-value").unwrap();
    let err = synthetic_cli(&target).snapshot(&target, &["model.default".into()]).err().unwrap();
    assert_eq!(err, HermesError::InvalidConfig); assert!(!format!("{err:?}").contains("secret-token-value"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_snapshot_treats_missing_config_as_empty_without_creating_it() {
    let root = temp(); let target = fake_cli(&root, "printf '%s\\n' \"$HERMES_HOME/config.yaml\"");
    fs::create_dir_all(&target.home).unwrap();
    let snapshot = synthetic_cli(&target).snapshot(&target, &["model.default".into()]).unwrap();
    assert!(matches!(snapshot.fields["model.default"], StoredValue::Absent));
    assert_eq!(snapshot.config_digest, format!("{:x}", Sha256::digest([])));
    assert_eq!(synthetic_cli(&target).raw_config(&target).unwrap(), None);
    assert!(!target.home.join("config.yaml").exists()); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_rejects_external_config_paths_and_failed_processes_without_output() {
    let root = temp(); let target = fake_cli(&root, "printf '%s\\n' /etc/passwd; echo secret-token-value >&2; exit 7");
    fs::create_dir_all(&target.home).unwrap();
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::CliFailed));
    let target = fake_cli(&root, "printf '%s\\n' /etc/passwd");
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::WrongTarget)); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_maps_missing_executable_to_fixed_error_category() {
    let root = temp(); let target = HermesTarget { executable: root.join("missing"), home: root.clone(), display_name: "missing".into() };
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::MissingInstallation)); fs::remove_dir_all(root).unwrap();
}
#[cfg(target_os = "linux")]
#[test]
fn cli_spawn_busy_executable_is_not_a_missing_installation() {
    let root = temp(); let target = fake_cli(&root, "exit 7");
    let writer = fs::OpenOptions::new().write(true).open(&target.executable).unwrap();
    assert_eq!(Command::new(&target.executable).spawn().unwrap_err().raw_os_error(), Some(libc::ETXTBSY));
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::CliFailed));
    drop(writer); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_spawn_permission_denied_is_not_a_missing_installation() {
    let root = temp(); let target = fake_cli(&root, "exit 7"); fs::set_permissions(&target.executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(Command::new(&target.executable).spawn().unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::CliFailed)); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_kills_a_process_that_exceeds_timeout() {
    let root = temp(); let target = fake_cli(&root, "exec sleep 20"); fs::create_dir_all(&target.home).unwrap();
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::Timeout)); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_success_does_not_wait_for_descendant_held_pipes() {
    let root = temp(); let target = fake_cli(&root, "printf '%s\\n' \"$HERMES_HOME/config.yaml\"; sleep 20 & echo $! > \"$HERMES_HOME/descendant.pid\"");
    fs::create_dir_all(&target.home).unwrap(); let start = Instant::now();
    assert!(synthetic_cli(&target).snapshot(&target, &[]).is_ok()); assert!(start.elapsed() < Duration::from_secs(3));
    assert!(target.home.join("descendant.pid").is_file()); fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_timeout_kills_descendant_and_reaps_direct_child() {
    let root = temp(); let target = fake_cli(&root, "sleep 20 & echo $! > \"$HERMES_HOME/descendant.pid\"; wait"); fs::create_dir_all(&target.home).unwrap();
    assert_eq!(synthetic_cli(&target).snapshot(&target, &[]).err(), Some(HermesError::Timeout));
    let pid: i32 = fs::read_to_string(target.home.join("descendant.pid")).unwrap().trim().parse().unwrap();
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) { assert_eq!(stat.rsplit_once(") ").unwrap().1.chars().next().unwrap(), 'Z'); }
    fs::remove_dir_all(root).unwrap();
}
