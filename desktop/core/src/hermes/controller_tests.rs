use super::*;
use super::controller::{Action, Controller, Inputs};
use super::store::HermesStore;
use std::{cell::RefCell, fs, time::Instant};
use serde_json::json;

const PROFILE: &str = "123E4567-E89B-12D3-A456-426614174000";
// Same ConfigAccess boundary as lifecycle tests; transitions/store/confirmations are real.
struct Fake {
    fields: RefCell<BTreeMap<String, Value>>,
    writes: RefCell<Vec<HermesTarget>>,
    before_write: RefCell<Option<Box<dyn Fn()>>>,
}
impl Fake {
    fn bytes(&self) -> Vec<u8> {
        let mut config = json!({});
        for (key, value) in self.fields.borrow().iter() {
            if let Some((section, field)) = key.split_once('.') {
                if config.get(section).is_none() { config[section] = json!({}); }
                config[section][field] = value.clone();
            } else { config[key] = value.clone(); }
        }
        serde_json::to_vec(&config).unwrap()
    }
}
impl ConfigAccess for Fake {
    fn capability(&self, _: &HermesTarget) -> cli::Capability { cli::Capability::Supported }
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
        Ok(ConfigSnapshot { config_path: target.home.join("config.yaml"), config_digest: service::digest(&self.bytes()), fields: keys.iter().map(|key| (key.clone(), self.fields.borrow().get(key).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent))).collect() })
    }
    fn raw_config(&self, _: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { Ok(Some(self.bytes())) }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> {
        if let Some(hook) = self.before_write.borrow().as_ref() { hook(); }
        self.writes.borrow_mut().push(target.clone()); self.fields.borrow_mut().insert(key.into(), value.clone()); Ok(())
    }
    fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> {
        if let Some(hook) = self.before_write.borrow().as_ref() { hook(); }
        self.writes.borrow_mut().push(target.clone()); self.fields.borrow_mut().remove(key); Ok(())
    }
}
struct Fixture { root: PathBuf, controller: Controller, inputs: Inputs, selection: HermesSelection, cli: Fake }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hermes-controller-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join("hermes"), b"synthetic").unwrap();
        let target = HermesTarget { executable: root.join("hermes"), home: root.join("home"), display_name: "synthetic".into() };
        let selection = HermesSelection { target: target.clone(), profile_id: PROFILE.into(), model_id: "team/model".into(), tenant: None };
        let inputs = Inputs { profiles: vec![Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/model".into())], policy: ManagedConfig::default(), disabled: false, helper: Some("/opt/keyzapper-helper".into()), targets: vec![target], diagnostics: vec![] };
        Self { controller: Controller::new(HermesStore::new(root.join("store"))), inputs, selection, root, cli: Fake { fields: RefCell::new(BTreeMap::from([("model.default".into(), json!("original"))])), writes: RefCell::new(Vec::new()), before_write: RefCell::new(None) } }
    }
}
impl Fixture {
    fn preview(&self, action: Action) -> controller::Preview {
        self.controller.begin().unwrap().preview(&self.cli, &self.inputs, action, &self.selection.target, if action == Action::Connect { Some(self.selection.clone()) } else { None }, Instant::now(), |_| {}).unwrap()
    }
    fn apply(&self, action: Action, id: &str) -> Result<(), String> {
        self.controller.begin().unwrap().apply(&self.cli, &self.inputs, action, &self.selection.target, if action == Action::Connect { Some(self.selection.clone()) } else { None }, id, Instant::now(), |_| {})
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

#[test]
fn tenant_model_mismatch_is_blocked_before_preview_and_apply_writes() {
    let mut fx = Fixture::new();
    fx.selection.tenant = Some("tenant-a".into());
    for model in ["tenant-b/sonnet", "sonnet", "tenant-aa/sonnet", "tenant-a/"] {
        fx.selection.model_id = model.into();
        assert!(fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), Instant::now(), |_| {}).is_err());
        assert!(fx.apply(Action::Connect, "untrusted-api-confirmation").is_err());
        assert!(fx.cli.writes.borrow().is_empty());
        assert!(fx.controller.store().load().unwrap().is_empty());
    }
    fx.selection.model_id = "tenant-a/sonnet".into();
    fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap();
    assert_eq!(fx.cli.fields.borrow()["model.default"], json!("tenant-a/sonnet"));
    assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
}

#[test]
fn configured_health_requires_the_stored_route_to_still_match_the_profile() {
    let mut fx = Fixture::new();
    fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().health, "configured");
    // An external edit re-pointed the profile while Hermes still names the old gateway.
    fx.inputs.profiles[0].endpoint = "https://other.test".into();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().health, "unavailable");
}

#[cfg(unix)]
#[test]
fn controller_revokes_changed_launcher_before_owner_snapshot_and_preserves_recovery() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap();
    let recorded = cli::RecordedVerifier::new(&fx.selection.target);
    assert_eq!(recorded.capability(&fx.selection.target), cli::Capability::Supported);
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    let marker = fx.root.join("config-path-executed");
    fs::write(&fx.selection.target.executable, format!("#!/bin/sh\ntouch '{}'\nprintf '%s\\n' \"$HERMES_HOME/config.yaml\"\n", marker.display())).unwrap();
    fs::set_permissions(&fx.selection.target.executable, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(recorded.capability(&fx.selection.target), cli::Capability::Unsupported);
    fx.controller.begin().unwrap().refresh(&cli::HermesCli::new(), &fx.inputs, |_| {}).unwrap();
    assert!(!marker.exists(), "unverified config path executed before capability");
    let view = fx.controller.view();
    assert!(!view.targets[0].supported);
    assert_eq!(view.selected.as_ref(), Some(&fx.selection));
    assert_eq!(view.health, "unavailable");
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
    // Private pending mapping remains visible even when executable support is revoked.
    let owner = fx.controller.store().load().unwrap().into_values().next().unwrap();
    let plan = binding::plan_connect(&fx.cli, fx.selection.clone(), &fx.inputs.profiles[0], fx.inputs.helper.as_deref().unwrap(), Some(&owner), &fx.inputs.policy).unwrap();
    let key = HermesStore::target_key(&fx.selection.target).unwrap();
    fx.controller.store().save_journal(&key, &store::Journal { plan, completed: vec![], phase: store::Phase::Prepared, operation: store::Operation::Connect }).unwrap();
    fx.controller.begin().unwrap().refresh(&cli::HermesCli::new(), &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().recovery_target.as_ref(), Some(&fx.selection.target));
    assert_eq!(fx.controller.view().health, "recoveryRequired");
    assert!(!marker.exists());
}

#[test]
fn controller_publishes_all_renewed_flags_even_when_supported_owner_snapshot_fails() {
    struct Failing<'a>(&'a Fake, &'a HermesTarget);
    impl ConfigAccess for Failing<'_> {
        fn capability(&self, target: &HermesTarget) -> cli::Capability { if target == self.1 { cli::Capability::Supported } else { cli::Capability::Unsupported } }
        fn snapshot(&self, _: &HermesTarget, _: &[String]) -> HermesResult<ConfigSnapshot> { Err(HermesError::CliFailed) }
        fn set(&self, t: &HermesTarget, k: &str, v: &Value) -> HermesResult<()> { self.0.set(t, k, v) }
        fn unset(&self, t: &HermesTarget, k: &str) -> HermesResult<()> { self.0.unset(t, k) }
    }
    let mut a = Fixture::new();
    let b = Fixture::new();
    a.inputs.targets.push(b.selection.target.clone());
    a.apply(Action::Connect, &a.preview(Action::Connect).confirmation_id).unwrap();
    assert!(a.controller.view().targets.iter().all(|entry| entry.supported));
    assert!(a.controller.begin().unwrap().refresh(&Failing(&a.cli, &a.selection.target), &a.inputs, |_| {}).is_err());
    let view = a.controller.view();
    assert!(view.targets[0].supported);
    assert!(!view.targets[1].supported, "stale supported flag survived snapshot failure");
    assert_eq!(view.selected.as_ref(), Some(&a.selection));
}

#[test]
fn controller_no_resolved_home_keeps_persisted_owner_and_journal_identity_without_cli() {
    let mut fx = Fixture::new();
    fx.controller.store().interrupt_on(store::StoreWrite::Cleanup, 1);
    let preview = fx.preview(Action::Connect);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.apply(Action::Connect, &preview.confirmation_id))).is_err());
    fx.inputs.targets.clear();
    fx.inputs.diagnostics = vec![cli::InstallationDiagnostic::for_executable(&fx.selection.target.executable)];
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    fx.controller.begin().unwrap().refresh(&cli::HermesCli::new(), &fx.inputs, |_| {}).unwrap();
    let view = fx.controller.view();
    assert_eq!(view.selected.as_ref(), Some(&fx.selection));
    assert_eq!(view.recovery_target.as_ref(), Some(&fx.selection.target));
    assert_eq!(view.health, "recoveryRequired");
    assert!(view.targets.is_empty());
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["diagnostics"][0].as_object().unwrap().keys().cloned().collect::<Vec<_>>(), vec!["executable", "reason"]);
    assert_eq!(json["diagnostics"][0]["reason"], "unknownBuild");
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
}

#[test]
fn controller_read_only_mapping_survives_removed_persisted_path_without_blocking_supported_target() {
    for mapping in ["owner", "journal"] {
        struct Tracked<'a> { fake: &'a Fake, calls: RefCell<Vec<HermesTarget>> }
        impl ConfigAccess for Tracked<'_> {
            fn capability(&self, target: &HermesTarget) -> cli::Capability { self.calls.borrow_mut().push(target.clone()); self.fake.capability(target) }
            fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> { self.calls.borrow_mut().push(target.clone()); self.fake.snapshot(target, keys) }
            fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { self.calls.borrow_mut().push(target.clone()); self.fake.raw_config(target) }
            fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.calls.borrow_mut().push(target.clone()); self.fake.set(target, key, value) }
            fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> { self.calls.borrow_mut().push(target.clone()); self.fake.unset(target, key) }
        }
        let mut fx = Fixture::new();
        let missing_root = fx.root.join("removed-target");
        fs::create_dir_all(missing_root.join("home")).unwrap();
        fs::write(missing_root.join("hermes"), b"synthetic-removed").unwrap();
        let missing = HermesTarget { executable: missing_root.join("hermes"), home: missing_root.join("home"), display_name: format!("removed-{mapping}") };
        let selection = HermesSelection { target: missing.clone(), profile_id: PROFILE.into(), model_id: "team/model".into(), tenant: None };
        let plan = binding::plan_connect(&fx.cli, selection.clone(), &fx.inputs.profiles[0], fx.inputs.helper.as_deref().unwrap(), None, &fx.inputs.policy).unwrap();
        let key = HermesStore::target_key(&missing).unwrap();
        match mapping {
            "owner" => fx.controller.store().save(&BTreeMap::from([(key, binding::ownership_after(&plan, None))])).unwrap(),
            "journal" => fx.controller.store().save_journal(&key, &store::Journal { plan, completed: vec![], phase: store::Phase::Prepared, operation: store::Operation::Connect }).unwrap(),
            _ => unreachable!(),
        }
        // Remove the actual persisted launcher path; the registry identity remains.
        fs::remove_file(&missing.executable).unwrap();
        fx.inputs.targets.push(fx.selection.target.clone());
        let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
        let access = Tracked { fake: &fx.cli, calls: RefCell::new(Vec::new()) };
        assert!(fx.controller.begin().unwrap().refresh(&access, &fx.inputs, |_| {}).is_ok(), "mapping={mapping}");
        let view = fx.controller.view();
        assert_eq!(view.health, if mapping == "journal" { "recoveryRequired" } else { "notConnected" }, "mapping={mapping}");
        assert!(view.problem.is_none(), "missing persisted target must not become a global error");
        assert_eq!(view.selected.as_ref().map(|s| &s.target), (mapping == "owner").then_some(&missing), "mapping={mapping}");
        assert_eq!(view.recovery_target.as_ref(), (mapping == "journal").then_some(&missing), "mapping={mapping}");
        assert!(view.targets.iter().any(|entry| entry.target == fx.selection.target && entry.supported));
        assert!(!view.targets.iter().any(|entry| entry.target == missing));
        assert!(access.calls.borrow().iter().all(|target| target == &fx.selection.target), "CLI/config access must never target removed B");
        assert!(fx.cli.writes.borrow().is_empty());
        assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
    }
}

#[test]
fn controller_refresh_publishes_verified_support_without_configured_owner_and_revokes_it() {
    let fx = Fixture::new();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    let view = fx.controller.view();
    assert!(view.selected.is_none());
    assert!(view.targets[0].supported, "support must come from verifier, not ownership or first-target default");
    assert!(fx.cli.writes.borrow().is_empty());
    fx.controller.begin().unwrap().refresh(&cli::HermesCli::new(), &fx.inputs, |_| {}).unwrap();
    let view = fx.controller.view();
    assert!(!view.targets[0].supported);
    assert!(view.problem.is_none());
    assert!(fx.cli.writes.borrow().is_empty());
}

#[test]
fn controller_connect_permit_blocks_profile_mutations_across_snapshots_until_io_finishes() {
    let fx = Fixture::new();
    let now = Instant::now();
    let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), now, |_| {}).unwrap();
    let other_snapshot = fx.controller.clone();
    let io_snapshot = other_snapshot.clone();
    *fx.cli.before_write.borrow_mut() = Some(Box::new(move || {
        assert!(io_snapshot.profile_change(Some(PROFILE), true).is_err());
        assert!(io_snapshot.profile_change(None, true).is_err());
        assert!(io_snapshot.profile_change(Some(PROFILE), false).is_err());
    }));
    let operation = fx.controller.begin().unwrap(); // BEFORE the model/profile snapshot.
    assert!(other_snapshot.profile_change(Some(PROFILE), true).is_err());
    assert!(other_snapshot.profile_change(None, true).is_err());
    let mut during_io = false;
    operation.apply(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), &preview.confirmation_id, now, |_| {
        during_io = true;
        assert!(other_snapshot.profile_change(Some(PROFILE), true).is_err());
    }).unwrap();
    assert!(during_io);
    assert!(other_snapshot.profile_change(Some(PROFILE), true).is_err(), "persisted owner must now block deletion");
    assert!(other_snapshot.profile_change(None, true).is_err(), "deactivation must see owner");
    assert!(other_snapshot.profile_change(Some("unrelated-profile"), true).is_ok(), "no phantom owner for unrelated deletion");
}

#[test]
fn controller_failed_connect_releases_profile_mutation_block_and_publishes_failure() {
    let fx = Fixture::new();
    let mut events = Vec::new();
    let operation = fx.controller.begin().unwrap();
    assert!(fx.controller.profile_change(None, true).is_err());
    assert!(operation.apply(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), "unknown", Instant::now(), |view| events.push(view)).is_err());
    assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
    assert_eq!(events.last().unwrap().health, "problem");
    assert!(events.last().unwrap().problem.is_some());
    assert!(fx.controller.profile_change(Some(PROFILE), true).is_ok());
    assert!(fx.controller.profile_change(None, true).is_ok());
    assert!(fx.cli.writes.borrow().is_empty());
}

#[test]
fn controller_preview_is_read_only_and_apply_publishes_real_persisted_selection() {
    let fx = Fixture::new();
    let now = Instant::now();
    let before = fx.cli.bytes();
    let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), now, |_| {}).unwrap();
    assert_eq!(fx.cli.bytes(), before);
    assert!(fx.cli.writes.borrow().is_empty());
    assert!(!fx.root.join("store/registry.json").exists());
    let mut events = Vec::new();
    fx.controller.begin().unwrap().apply(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), &preview.confirmation_id, now, |view| events.push(view)).unwrap();
    assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
    assert_eq!(events.last().unwrap().selected.as_ref(), Some(&fx.selection));
    assert_eq!(events.last().unwrap().health, "configured");
    assert!(fx.cli.writes.borrow().iter().all(|target| target == &fx.selection.target));
    let owner = fx.controller.store().owner(&HermesStore::target_key(&fx.selection.target).unwrap()).unwrap().unwrap();
    assert_eq!(owner.selection, fx.selection);
}

#[test]
fn controller_refresh_surfaces_corrupt_store_and_never_mutates_pending_recovery() {
    let fx = Fixture::new();
    fx.controller.store().interrupt_on(store::StoreWrite::Cleanup, 1);
    let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), Instant::now(), |_| {}).unwrap();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fx.controller.begin().unwrap().apply(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), &preview.confirmation_id, Instant::now(), |_| {})
    })).is_err());
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    let writes = fx.cli.writes.borrow().len();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().health, "recoveryRequired");
    assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
    assert_eq!(fx.cli.writes.borrow().len(), writes);
    assert!(fx.controller.profile_change(Some(PROFILE), true).is_err());
    fs::write(fx.root.join("store/registry.json"), b"synthetic-secret-invalid-registry").unwrap();
    let mut events = Vec::new();
    assert!(fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |view| events.push(view)).is_err());
    assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
    assert_eq!(events.last().unwrap().health, "problem");
    assert!(events.last().unwrap().problem.as_ref().unwrap().contains("unreadable"));
    assert!(!serde_json::to_string(events.last().unwrap()).unwrap().contains("synthetic-secret"));
    assert!(fx.controller.profile_change(Some("unrelated"), true).is_err());
    assert!(fx.controller.profile_change(None, true).is_err());
}

#[test]
fn controller_routes_disconnect_to_real_lifecycle_and_refreshes_selection() {
    assert_eq!(Action::parse("connect").unwrap(), Action::Connect);
    assert_eq!(Action::parse("disconnect").unwrap(), Action::Disconnect);
    assert_eq!(Action::parse("recover").unwrap(), Action::Recover);
    assert!(Action::parse("unknown").is_err());
    let fx = Fixture::new();
    let original = fx.cli.bytes();
    fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
    let writes = fx.cli.writes.borrow().len();
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    let preview = fx.preview(Action::Disconnect);
    assert_eq!(fx.cli.writes.borrow().len(), writes);
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
    fx.apply(Action::Disconnect, &preview.confirmation_id).unwrap();
    assert_eq!(fx.cli.bytes(), original);
    assert_eq!(fx.controller.view().selected, None);
    assert_eq!(fx.controller.view().health, "notConnected");
    assert!(fx.apply(Action::Disconnect, &preview.confirmation_id).is_err());
    assert!(fx.controller.profile_change(Some(PROFILE), true).is_ok());
}

#[test]
fn controller_routes_recovery_through_actual_rollback_and_cleanup_phases() {
    for phase in [store::StoreWrite::Journal, store::StoreWrite::Cleanup] {
        let fx = Fixture::new();
        let original = fx.cli.bytes();
        fx.controller.store().interrupt_on(phase, if phase == store::StoreWrite::Journal { 4 } else { 1 });
        let preview = fx.preview(Action::Connect);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.apply(Action::Connect, &preview.confirmation_id))).is_err());
        let committed = fx.cli.bytes();
        let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
        let writes = fx.cli.writes.borrow().len();
        let preview = fx.preview(Action::Recover);
        assert_eq!(fx.cli.writes.borrow().len(), writes);
        assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
        assert!(fx.apply(Action::Disconnect, &preview.confirmation_id).is_err());
        fx.apply(Action::Recover, &preview.confirmation_id).unwrap();
        assert!(!fx.controller.store().has_pending_operation().unwrap());
        if phase == store::StoreWrite::Journal {
            assert_eq!(fx.cli.bytes(), original);
            assert_eq!(fx.controller.view().selected, None);
            assert_eq!(fx.controller.view().health, "notConnected");
        } else {
            assert_eq!(fx.cli.bytes(), committed);
            assert_eq!(fx.cli.writes.borrow().len(), writes);
            assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
            assert_eq!(fx.controller.view().health, "configured");
        }
        let recovered = fx.cli.bytes();
        let recovered_writes = fx.cli.writes.borrow().len();
        let registry_after = fs::read(fx.root.join("store/registry.json")).unwrap();
        assert!(fx.apply(Action::Recover, &preview.confirmation_id).is_err());
        assert_eq!(fx.cli.bytes(), recovered);
        assert_eq!(fx.cli.writes.borrow().len(), recovered_writes);
        assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry_after);
    }
}

#[test]
fn controller_revalidates_selection_policy_helper_inventory_and_config_before_writes() {
    for changed in ["profile", "endpoint", "policy", "helper", "inventory", "disabled", "model", "tenant", "target", "config", "expired"] {
        let mut fx = Fixture::new();
        let now = Instant::now();
        let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), now, |_| {}).unwrap();
        let mut apply_time = now;
        match changed {
            "profile" => fx.inputs.profiles.clear(),
            "endpoint" => fx.inputs.profiles[0].endpoint = "https://other.test".into(),
            "policy" => fx.inputs.policy.allowed_gateway_hosts = vec!["other.test".into()],
            "helper" => fx.inputs.helper = Some("/opt/moved/keyzapper-helper".into()),
            "inventory" => fx.inputs.targets.clear(),
            "disabled" => fx.inputs.disabled = true,
            "model" => fx.selection.model_id = "other-model".into(),
            "tenant" => fx.selection.tenant = Some("other-tenant".into()),
            "target" => { fs::create_dir(fx.root.join("other-home")).unwrap(); fx.selection.target.home = fx.root.join("other-home"); fx.inputs.targets.push(fx.selection.target.clone()); }
            "config" => { fx.cli.fields.borrow_mut().insert("theme".into(), json!("changed")); }
            "expired" => apply_time += std::time::Duration::from_secs(60),
            _ => unreachable!(),
        }
        let before = fx.cli.bytes();
        let mut events = Vec::new();
        let result = fx.controller.begin().unwrap().apply(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), &preview.confirmation_id, apply_time, |view| events.push(view));
        assert!(result.is_err(), "accepted stale {changed}");
        assert!(fx.cli.writes.borrow().is_empty(), "wrote stale {changed}");
        assert_eq!(fx.cli.bytes(), before);
        assert!(!fx.root.join("store/registry.json").exists());
        assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
        assert!(fx.controller.begin().is_ok());
    }
}

#[test]
fn controller_replayed_connect_never_writes_again() {
    let fx = Fixture::new();
    let preview = fx.preview(Action::Connect);
    fx.apply(Action::Connect, &preview.confirmation_id).unwrap();
    let before = fx.cli.bytes();
    let writes = fx.cli.writes.borrow().len();
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    assert!(fx.apply(Action::Connect, &preview.confirmation_id).is_err());
    assert_eq!(fx.cli.bytes(), before);
    assert_eq!(fx.cli.writes.borrow().len(), writes);
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
    assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
}

#[test]
fn controller_confirmation_expiring_during_preflight_is_refused_before_writes() {
    struct Slow<'a>(&'a Fake);
    impl ConfigAccess for Slow<'_> {
        fn capability(&self, target: &HermesTarget) -> cli::Capability { self.0.capability(target) }
        fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
            std::thread::sleep(std::time::Duration::from_millis(3));
            self.0.snapshot(target, keys)
        }
        fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { self.0.raw_config(target) }
        fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.0.set(target, key, value) }
        fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> { self.0.unset(target, key) }
    }
    let fx = Fixture::new();
    let now = Instant::now();
    let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), now, |_| {}).unwrap();
    let result = fx.controller.begin().unwrap().apply(&Slow(&fx.cli), &fx.inputs, Action::Connect, &fx.selection.target, Some(fx.selection.clone()), &preview.confirmation_id, now + std::time::Duration::from_millis(59999), |_| {});
    assert!(result.is_err(), "expired during config preflight but still applied");
    assert!(fx.cli.writes.borrow().is_empty());
    assert!(!fx.root.join("store/registry.json").exists());
}

struct Pair<'a> { first: &'a Fixture, second: &'a Fixture }
impl Pair<'_> {
    fn cli(&self, target: &HermesTarget) -> &Fake {
        if target == &self.first.selection.target { &self.first.cli }
        else { assert_eq!(target, &self.second.selection.target); &self.second.cli }
    }
}
impl ConfigAccess for Pair<'_> {
    fn capability(&self, target: &HermesTarget) -> cli::Capability { self.cli(target).capability(target) }
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> { self.cli(target).snapshot(target, keys) }
    fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { self.cli(target).raw_config(target) }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.cli(target).set(target, key, value) }
    fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> { self.cli(target).unset(target, key) }
}

#[cfg(unix)]
#[test]
fn controller_mixed_inventory_keeps_exact_supported_recovery_actionable_on_real_page() {
    struct Mixed<'a>(&'a Fixture, &'a cli::RecordedVerifier);
    impl ConfigAccess for Mixed<'_> {
        fn capability(&self, target: &HermesTarget) -> cli::Capability {
            self.1.capability(target)
        }
        fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> { assert_eq!(target, &self.0.selection.target); self.0.cli.snapshot(target, keys) }
        fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { assert_eq!(target, &self.0.selection.target); self.0.cli.raw_config(target) }
        fn set(&self, t: &HermesTarget, k: &str, v: &Value) -> HermesResult<()> { self.0.cli.set(t, k, v) }
        fn unset(&self, t: &HermesTarget, k: &str) -> HermesResult<()> { self.0.cli.unset(t, k) }
    }
    let mut a = Fixture::new();
    let b = Fixture::new();
    let plan = binding::plan_connect(&a.cli, a.selection.clone(), &a.inputs.profiles[0], a.inputs.helper.as_deref().unwrap(), None, &a.inputs.policy).unwrap();
    let recorded = cli::RecordedVerifier::new(&a.selection.target);
    let key = HermesStore::target_key(&a.selection.target).unwrap();
    a.controller.store().save_journal(&key, &store::Journal { plan, completed: vec![], phase: store::Phase::Prepared, operation: store::Operation::Connect }).unwrap();
    a.inputs.targets.push(b.selection.target.clone());
    let registry = fs::read(a.root.join("store/registry.json")).unwrap();
    a.controller.begin().unwrap().refresh(&Mixed(&a, &recorded), &a.inputs, |_| {}).unwrap();
    let view = a.controller.view();
    assert_eq!(view.health, "recoveryRequired");
    assert!(view.problem.is_none(), "target availability is not global registry corruption");
    assert!(view.selected.is_none());
    assert_eq!(view.recovery_target.as_ref(), Some(&a.selection.target));
    assert!(view.targets[0].supported); assert!(!view.targets[1].supported);
    assert_eq!(fs::read(a.root.join("store/registry.json")).unwrap(), registry);
    assert!(a.cli.writes.borrow().is_empty());
    // Minimal seam: real serialized controller output feeds the actual JS page.
    let output = std::process::Command::new("node")
        .args(["--test", "--test-name-pattern=controller mixed inventory", "../ui/tests/hermes.test.mjs"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("KEYZAPPER_HERMES_CONTROLLER_VIEW", serde_json::to_string(&view).unwrap()).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let preview = a.controller.begin().unwrap().preview(&Mixed(&a, &recorded), &a.inputs, Action::Recover, &a.selection.target, None, Instant::now(), |_| {}).unwrap();
    assert!(!preview.confirmation_id.is_empty());
    assert!(a.controller.begin().unwrap().preview(&Mixed(&a, &recorded), &a.inputs, Action::Recover, &b.selection.target, None, Instant::now(), |_| {}).is_err());
}

#[test]
fn controller_exact_target_routing_preserves_other_config_owner_and_selection() {
    let mut a = Fixture::new();
    let b = Fixture::new();
    a.inputs.targets.push(b.selection.target.clone());
    let pair = Pair { first: &a, second: &b };
    let original_a = a.cli.bytes();
    let original_b = b.cli.bytes();
    let preview = a.controller.begin().unwrap().preview(&pair, &a.inputs, Action::Connect, &b.selection.target, Some(b.selection.clone()), Instant::now(), |_| {}).unwrap();
    assert!(a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Connect, &a.selection.target, Some(a.selection.clone()), &preview.confirmation_id, Instant::now(), |_| {}).is_err());
    assert_eq!(a.cli.bytes(), original_a);
    assert_eq!(b.cli.bytes(), original_b);
    a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Connect, &b.selection.target, Some(b.selection.clone()), &preview.confirmation_id, Instant::now(), |_| {}).unwrap();
    assert_eq!(a.cli.bytes(), original_a);
    let key_b = HermesStore::target_key(&b.selection.target).unwrap();
    let owner_b = a.controller.store().owner(&key_b).unwrap();
    let committed_b = b.cli.bytes();
    let writes_b = b.cli.writes.borrow().len();
    let preview_a = a.controller.begin().unwrap().preview(&pair, &a.inputs, Action::Connect, &a.selection.target, Some(a.selection.clone()), Instant::now(), |_| {}).unwrap();
    a.controller.store().interrupt_on(store::StoreWrite::Cleanup, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Connect, &a.selection.target, Some(a.selection.clone()), &preview_a.confirmation_id, Instant::now(), |_| {})
    })).is_err());
    let recover_a = a.controller.begin().unwrap().preview(&pair, &a.inputs, Action::Recover, &a.selection.target, None, Instant::now(), |_| {}).unwrap();
    let recovery_writes = a.cli.writes.borrow().len();
    assert!(a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Recover, &b.selection.target, None, &recover_a.confirmation_id, Instant::now(), |_| {}).is_err());
    a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Recover, &a.selection.target, None, &recover_a.confirmation_id, Instant::now(), |_| {}).unwrap();
    assert_eq!(a.cli.writes.borrow().len(), recovery_writes, "cleanup must not undo committed state");
    let disconnect_a = a.controller.begin().unwrap().preview(&pair, &a.inputs, Action::Disconnect, &a.selection.target, None, Instant::now(), |_| {}).unwrap();
    assert!(a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Disconnect, &b.selection.target, None, &disconnect_a.confirmation_id, Instant::now(), |_| {}).is_err());
    a.controller.begin().unwrap().apply(&pair, &a.inputs, Action::Disconnect, &a.selection.target, None, &disconnect_a.confirmation_id, Instant::now(), |_| {}).unwrap();
    assert_eq!(a.cli.bytes(), original_a);
    assert_eq!(b.cli.bytes(), committed_b);
    assert_eq!(b.cli.writes.borrow().len(), writes_b);
    assert!(a.controller.store().owner(&key_b).unwrap() == owner_b);
    assert_eq!(a.controller.view().selected.as_ref(), Some(&b.selection));
}

#[test]
fn controller_disconnect_and_recover_refuse_stale_and_expired_scopes_without_writes() {
    for action in [Action::Disconnect, Action::Recover] {
        for changed in ["config", "policy", "profile", "expired", "wrong-action"] {
            let mut fx = Fixture::new();
            if action == Action::Recover { fx.controller.store().interrupt_on(store::StoreWrite::Cleanup, 1); }
            let connect = fx.preview(Action::Connect);
            let connected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.apply(Action::Connect, &connect.confirmation_id)));
            if action == Action::Recover { assert!(connected.is_err()); } else { connected.unwrap().unwrap(); }
            let now = Instant::now();
            let preview = fx.controller.begin().unwrap().preview(&fx.cli, &fx.inputs, action, &fx.selection.target, None, now, |_| {}).unwrap();
            let mut at = now;
            let mut applied_action = action;
            match changed {
                "config" => { fx.cli.fields.borrow_mut().insert("theme".into(), json!("edited")); }
                "policy" => fx.inputs.policy.allowed_gateway_hosts = vec!["gateway.test".into()],
                "profile" => fx.inputs.profiles.clear(),
                "expired" => at += std::time::Duration::from_secs(60),
                "wrong-action" => applied_action = if action == Action::Recover { Action::Disconnect } else { Action::Recover },
                _ => unreachable!(),
            }
            let before = fx.cli.bytes();
            let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
            let writes = fx.cli.writes.borrow().len();
            let mut events = Vec::new();
            assert!(fx.controller.begin().unwrap().apply(&fx.cli, &fx.inputs, applied_action, &fx.selection.target, None, &preview.confirmation_id, at, |view| events.push(view)).is_err());
            assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
            assert_eq!(fx.cli.bytes(), before);
            assert_eq!(fx.cli.writes.borrow().len(), writes);
            assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
            assert!(fx.controller.begin().is_ok());
        }
    }
}

#[test]
fn controller_removed_profile_refresh_preserves_bound_id_without_fallback_or_writes() {
    let mut fx = Fixture::new();
    fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap();
    let writes = fx.cli.writes.borrow().len();
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    fx.inputs.profiles[0].id = "123E4567-E89B-12D3-A456-426614174001".into();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    assert_eq!(fx.controller.view().health, "unavailable");
    assert_eq!(fx.controller.view().selected.as_ref(), Some(&fx.selection));
    assert_eq!(fx.cli.writes.borrow().len(), writes);
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
}

#[test]
fn controller_lifecycle_errors_and_unsupported_capability_always_reset_and_publish_busy() {
    struct Denied<'a> { inner: &'a Fake, failure: &'static str }
    impl ConfigAccess for Denied<'_> {
        fn capability(&self, target: &HermesTarget) -> cli::Capability {
            if self.failure == "capability" { cli::Capability::Unsupported } else { self.inner.capability(target) }
        }
        fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
            if self.failure == "snapshot" { Err(HermesError::CliFailed) } else { self.inner.snapshot(target, keys) }
        }
        fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> {
            if self.failure == "raw" { Err(HermesError::CliFailed) } else { self.inner.raw_config(target) }
        }
        fn set(&self, _: &HermesTarget, _: &str, _: &Value) -> HermesResult<()> { Err(HermesError::Unsupported) }
        fn unset(&self, _: &HermesTarget, _: &str) -> HermesResult<()> { Err(HermesError::Unsupported) }
    }
    for action in [Action::Connect, Action::Disconnect, Action::Recover] {
        for failure in ["snapshot", "raw", "capability", "journal"] {
            let fx = Fixture::new();
            if action == Action::Disconnect { fx.apply(Action::Connect, &fx.preview(Action::Connect).confirmation_id).unwrap(); }
            if action == Action::Recover {
                fx.controller.store().interrupt_on(store::StoreWrite::Journal, 4);
                let preview = fx.preview(Action::Connect);
                assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.apply(Action::Connect, &preview.confirmation_id))).is_err());
            }
            let preview = fx.preview(action);
            if failure == "journal" {
                fx.controller.store().fail_on(if action == Action::Recover { store::StoreWrite::Commit } else { store::StoreWrite::Journal }, 1);
            }
            let denied = Denied { inner: &fx.cli, failure };
            let access: &dyn ConfigAccess = if failure == "journal" { &fx.cli } else { &denied };
            let mut events = Vec::new();
            let result = fx.controller.begin().unwrap().apply(access, &fx.inputs, action, &fx.selection.target, if action == Action::Connect { Some(fx.selection.clone()) } else { None }, &preview.confirmation_id, Instant::now(), |view| events.push(view));
            assert!(result.is_err(), "unexpected success {action:?}/{failure}");
            assert_eq!(events.iter().map(|v| v.busy).collect::<Vec<_>>(), vec![true, false]);
            assert_eq!(events.last().unwrap().health, "problem");
            assert!(events.last().unwrap().problem.is_some());
            assert!(fx.controller.begin().is_ok());
        }
    }
}

#[test]
fn controller_serialization_contains_only_nonsecret_view_and_preview_fields() {
    let fx = Fixture::new();
    fx.cli.fields.borrow_mut().insert("unrelated-secret".into(), json!("synthetic-credential-marker"));
    let preview = fx.preview(Action::Connect);
    let preview_json = serde_json::to_value(&preview).unwrap();
    let view_json = serde_json::to_value(fx.controller.view()).unwrap();
    assert_eq!(preview_json.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), vec!["confirmationId", "expiresInSeconds", "summary"]);
    assert_eq!(view_json.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), vec!["busy", "diagnostics", "health", "problem", "recoveryTarget", "selected", "targets", "testStatus"]);
    assert!(view_json["recoveryTarget"].is_null());
    // Include a real pending journal with private original values in the refresh path.
    let plan = binding::plan_connect(&fx.cli, fx.selection.clone(), &fx.inputs.profiles[0], fx.inputs.helper.as_deref().unwrap(), None, &fx.inputs.policy).unwrap();
    let journal = store::Journal { plan, completed: vec![], phase: store::Phase::Prepared, operation: store::Operation::Connect };
    let key = HermesStore::target_key(&fx.selection.target).unwrap();
    fx.controller.store().save_journal(&key, &journal).unwrap();
    fx.controller.begin().unwrap().refresh(&fx.cli, &fx.inputs, |_| {}).unwrap();
    let recovery_json = serde_json::to_value(fx.controller.view()).unwrap();
    assert_eq!(recovery_json["recoveryTarget"], serde_json::to_value(&fx.selection.target).unwrap());
    assert_eq!(recovery_json["recoveryTarget"].as_object().unwrap().keys().cloned().collect::<Vec<_>>(), vec!["displayName", "executable", "home"]);
    assert_eq!(view_json["testStatus"], "notTested");
    let serialized = format!("{preview_json}{view_json}{recovery_json}");
    for forbidden in ["synthetic-credential-marker", "last_written", "lastWritten", "original", "key_cmd", "config_digest", "backup", "history", "journal"] { assert!(!serialized.contains(forbidden)); }
}
