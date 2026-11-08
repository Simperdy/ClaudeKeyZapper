//! Synthetic transport tests of the production verifier, not acceptance evidence.
use super::*;
use serde_json::json;

struct Installation { _scratch: ProbeHome, target: HermesTarget, root: PathBuf, fixture: Value, diagnostic: Value }
/// Private recorded transport, not an additional supported build or real CLI.
pub(crate) struct RecordedVerifier(Installation);
impl RecordedVerifier {
    pub(crate) fn new(target: &HermesTarget) -> Self {
        let mut installation = Installation::new();
        fs::copy(&installation.target.executable, &target.executable).unwrap();
        installation.target = target.clone();
        // The recorded entrypoint follows the target this transport actually serves.
        installation.fixture["supportedBuilds"][0]["nativeResult"]["selected_executable"] =
            json!(fs::canonicalize(&target.executable).unwrap());
        Self(installation)
    }
    pub(crate) fn capability(&self, target: &HermesTarget) -> Capability {
        if verify_installation(target, &self.0.fixture, |command| Ok(self.0.response(&command, &self.0.diagnostic))).is_ok() {
            Capability::Supported
        } else { Capability::Unsupported }
    }
}
impl Installation {
    fn new() -> Self {
        let scratch = ProbeHome::new().unwrap();
        let root = scratch.0.join("source");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hermes_bootstrap.py"), "# public source fixture\n").unwrap();
        let target = HermesTarget { executable: scratch.0.join("hermes-cli"), home: scratch.0.join("hermes"), display_name: "fixture".into() };
        // Never executed: the private transport captures and validates every command.
        fs::write(&target.executable, "#!/bin/sh\n# from hermes_cli.main import main\n").unwrap();
        let entrypoint = format!("{:x}", Sha256::digest(fs::read(&target.executable).unwrap()));
        let mut fixture: Value = serde_json::from_str(COMPATIBILITY_FIXTURE).unwrap();
        fixture["publicSourceFiles"] = json!(["hermes_bootstrap.py"]);
        let tree = public_source_fingerprint(&root, &fixture).unwrap();
        let build = &mut fixture["supportedBuilds"][0];
        build["entrypointSha256"] = json!(entrypoint);
        build["nativeResult"]["entrypoint_sha256"] = json!(entrypoint);
        build["nativeResult"]["selected_executable"] = json!(fs::canonicalize(&target.executable).unwrap());
        build["nativeResult"]["source_root"] = json!(root);
        build["sourceTreeSha256"] = json!(tree);
        build["nativeResult"]["local_diagnostic"]["sourceTreeSha256"] = json!(tree);
        let diagnostic = build["nativeResult"]["local_diagnostic"].clone();
        Self { _scratch: scratch, target, root, fixture, diagnostic }
    }
    fn response(&self, command: &Command, diagnostic: &Value) -> Vec<u8> {
        let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
        let env: std::collections::BTreeMap<_, _> = command.get_envs().map(|(k,v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap())).collect();
        assert!(env.contains_key("PYTHONPYCACHEPREFIX"), "CLI imports must not execute unverified installation bytecode");
        assert_eq!(env.len(), 8);
        assert!(!env.contains_key("HERMES_PROFILE"));
        assert!(env["HOME"].contains("keyzapper-hermes-probe-"));
        assert_ne!(Path::new(env["HERMES_HOME"]), self.target.home);
        match args[0] {
            "--version" => format!("Hermes Agent vunknown (2026.9.24)\nInstall directory: {}\n", self.root.display()).into_bytes(),
            "-I" => {
                assert_eq!(command.get_program(), "/bin/sh", "must use selected absolute shebang, never PATH python");
                assert_eq!(args[1], "-B");
                serde_json::to_vec(diagnostic).unwrap()
            }
            "config" => { assert_eq!(args[2], "--help"); format!("usage: hermes config {}\n", args[1]).into_bytes() }
            _ => panic!("unexpected discovery command"),
        }
    }
}

#[test]
fn copied_byte_identical_launcher_is_denied_before_first_execution() {
    let install = Installation::new();
    let shadow = install._scratch.0.join("shadow");
    fs::create_dir(&shadow).unwrap();
    let mut target = install.target.clone();
    target.executable = shadow.join("hermes");
    fs::copy(&install.target.executable, &target.executable).unwrap();
    let mut executions = 0;
    assert!(verify_installation(&target, &install.fixture, |cmd| {
        executions += 1;
        Ok(install.response(&cmd, &install.diagnostic))
    }).is_err());
    assert_eq!(executions, 0, "copied launcher's import context must never execute");
}

#[test]
fn production_verifier_requires_exact_source_runtime_diagnostic_and_cli_methods() {
    let install = Installation::new();
    let verify = |diagnostic: &Value| verify_installation(&install.target, &install.fixture, |cmd| Ok(install.response(&cmd, diagnostic)));
    assert_eq!(verify(&install.diagnostic), Ok(()));
    for key in ["passed", "sdkJsonSseToolParsing"] {
        let mut bad = install.diagnostic.clone(); bad[key] = json!(false);
        assert!(verify(&bad).is_err());
    }
    for key in ["sourceTreeSha256", "pythonVersion", "anthropicVersion"] {
        let mut bad = install.diagnostic.clone(); bad[key] = json!("mismatch");
        assert!(verify(&bad).is_err());
        bad.as_object_mut().unwrap().remove(key);
        assert!(verify(&bad).is_err());
    }
    assert!(verify(&Value::Null).is_err());
    for bad_output in [b"not JSON".to_vec(), b"{}".to_vec()] {
        assert!(verify_installation(&install.target, &install.fixture, |cmd| {
            if cmd.get_args().next().unwrap() == "-I" { Ok(bad_output.clone()) }
            else { Ok(install.response(&cmd, &install.diagnostic)) }
        }).is_err());
    }
    for fail_at in ["-I", "config"] {
        assert!(verify_installation(&install.target, &install.fixture, |cmd| {
            if cmd.get_args().next().unwrap() == fail_at { Err(HermesError::CliFailed) }
            else { Ok(install.response(&cmd, &install.diagnostic)) }
        }).is_err());
    }
}

#[test]
fn production_verifier_refuses_partial_evidence_before_executing_anything() {
    let install = Installation::new();
    for pointer in ["/fullPass", "/checks/selected_launcher_chat", "/nativeResult/checks/selected_launcher_chat"] {
        let mut fixture = install.fixture.clone();
        *fixture["supportedBuilds"][0].pointer_mut(pointer).unwrap() = json!(false);
        assert!(verify_installation(&install.target, &fixture, |_| panic!("partial evidence executed")).is_err());
    }
    fs::write(&install.target.executable, "#!/bin/sh\necho 'Hermes Agent vunknown (2026.9.24)'\n").unwrap();
    assert!(verify_installation(&install.target, &install.fixture, |_| panic!("unknown launcher executed")).is_err());
    assert_eq!(HermesCli::new().check_write(&install.target), Err(HermesError::Unsupported));
    assert!(!install.target.home.join("config.yaml").exists());
}

#[test]
fn production_verifier_rechecks_diagnostic_and_detects_source_and_launcher_drift() {
    let install = Installation::new();
    assert!(verify_installation(&install.target, &install.fixture, |cmd| Ok(install.response(&cmd, &install.diagnostic))).is_ok());
    // A previous pass does not replace a fresh diagnostic.
    assert!(verify_installation(&install.target, &install.fixture, |cmd| {
        if cmd.get_args().next().unwrap() == "-I" { Err(HermesError::CliFailed) }
        else { Ok(install.response(&cmd, &install.diagnostic)) }
    }).is_err());
    assert!(verify_installation(&install.target, &install.fixture, |cmd| {
        let response = install.response(&cmd, &install.diagnostic);
        if cmd.get_args().next().unwrap() == "-I" { fs::write(&install.target.executable, "changed").unwrap(); }
        Ok(response)
    }).is_err());
    let install = Installation::new();
    fs::write(install.root.join("hermes_bootstrap.py"), "changed").unwrap();
    assert!(verify_installation(&install.target, &install.fixture, |_| panic!("changed installed source executed even for --version")).is_err());
}

#[cfg(unix)]
#[test]
fn production_source_manifest_rejects_symlinks_and_path_escape_without_reading_target() {
    let install = Installation::new();
    fs::remove_file(install.root.join("hermes_bootstrap.py")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/private/.env", install.root.join("hermes_bootstrap.py")).unwrap();
    assert!(public_source_fingerprint(&install.root, &install.fixture).is_err());
    for path in ["../.env", "/etc/passwd"] {
        let mut fixture = install.fixture.clone(); fixture["publicSourceFiles"] = json!([path]);
        assert!(public_source_fingerprint(&install.root, &fixture).is_err());
    }
}
