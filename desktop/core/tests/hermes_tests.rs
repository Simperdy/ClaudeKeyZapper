use keyzapper_core::hermes::{ConfigAccess, HermesError, HermesTarget, StoredValue};
use serde_json::{json, Value};
use sha2::Digest;
use std::{cell::RefCell, collections::BTreeMap, fs, path::PathBuf};
#[cfg(unix)]
use keyzapper_core::hermes::cli::HermesCli;
#[cfg(unix)]
use std::path::Path;
use keyzapper_core::hermes::{binding::{desired_provider, plan_connect, HermesSelection}, store::HermesStore};
use keyzapper_core::hermes::{connect, disconnect, inspect, recover, BindingHealth};
use keyzapper_core::{managed::ManagedConfig, models::{AuthType, Profile}};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

struct FakeConfig {
    values: RefCell<BTreeMap<String, Value>>,
    calls: RefCell<Vec<String>>,
    fail_write: RefCell<Option<usize>>,
    write_count: RefCell<usize>,
    backup_to_observe: RefCell<Option<PathBuf>>,
    backup_seen_before_write: RefCell<bool>,
    after_write: RefCell<Option<Box<dyn Fn(&FakeConfig, usize)>>>,
}
impl FakeConfig {
    fn new(values: BTreeMap<String, Value>) -> Self { Self { values: RefCell::new(values), calls: RefCell::new(Vec::new()), fail_write: RefCell::new(None), write_count: RefCell::new(0), backup_to_observe: RefCell::new(None), backup_seen_before_write: RefCell::new(false), after_write: RefCell::new(None) } }
    fn fail_write_number(&self, n: usize) { *self.fail_write.borrow_mut() = Some(n); }
    fn all_stored(&self) -> BTreeMap<String, Value> { self.values.borrow().clone() }
    fn bytes(&self) -> Vec<u8> {
        let mut config = json!({});
        for (key, value) in self.values.borrow().iter() {
            if let Some((section, field)) = key.split_once('.') {
                if config.get(section).is_none() { config[section] = json!({}); }
                config[section][field] = value.clone();
            } else { config[key] = value.clone(); }
        }
        serde_json::to_vec(&config).unwrap()
    }
    fn write(&self, target: &HermesTarget, key: &str, value: Option<&Value>) -> keyzapper_core::hermes::HermesResult<()> {
        let mut count = self.write_count.borrow_mut(); *count += 1;
        self.calls.borrow_mut().push(format!("write:{}", target.home.display()));
        if let Some(path) = self.backup_to_observe.borrow().as_ref() { *self.backup_seen_before_write.borrow_mut() = path.is_file(); }
        if *self.fail_write.borrow() == Some(*count) { return Err(HermesError::CliFailed); }
        if let Some(value) = value { self.values.borrow_mut().insert(key.into(), value.clone()); } else { self.values.borrow_mut().remove(key); }
        let number = *count;
        drop(count);
        if let Some(hook) = self.after_write.borrow().as_ref() { hook(self, number); }
        Ok(())
    }
}
impl ConfigAccess for FakeConfig {
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> keyzapper_core::hermes::HermesResult<keyzapper_core::hermes::ConfigSnapshot> {
        self.calls.borrow_mut().push(format!("read:{}", target.home.display()));
        let fields = keys.iter().map(|key| (key.clone(), self.values.borrow().get(key).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent))).collect();
        let bytes = self.bytes();
        let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
        Ok(keyzapper_core::hermes::ConfigSnapshot { config_path: target.home.join("config.yaml"), config_digest: digest, fields })
    }
    fn raw_config(&self, _target: &HermesTarget) -> keyzapper_core::hermes::HermesResult<Option<Vec<u8>>> {
        Ok(Some(self.bytes()))
    }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> keyzapper_core::hermes::HermesResult<()> {
        self.write(target, key, Some(value))
    }
    fn unset(&self, target: &HermesTarget, key: &str) -> keyzapper_core::hermes::HermesResult<()> {
        self.write(target, key, None)
    }
}

fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("hermes task3 {}", uuid::Uuid::new_v4())); fs::create_dir_all(&p).unwrap(); p
}
#[cfg(unix)]
fn fake_cli(root: &Path, script: &str) -> HermesTarget {
    let exe = root.join("fake hermes; $(touch pwned)'"); fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    HermesTarget { executable: exe, home: root.join("selected home; $(touch home-pwned)'") , display_name: "test".into() }
}

const ALPHA: &str = "123E4567-E89B-12D3-A456-426614174000";
const BETA: &str = "123E4567-E89B-12D3-A456-426614174001";
fn profile(id: &str) -> Profile { Profile::new(id.into(), "Work".into(), "https://gw.example.test".into(), "team/sonnet".into()) }
fn selection(id: &str) -> HermesSelection { HermesSelection { target: HermesTarget { executable: "/usr/bin/hermes".into(), home: "/tmp/hermes-home".into(), display_name: "Hermes".into() }, profile_id: id.into(), model_id: "team/sonnet".into(), tenant: Some("team".into()) } }
fn current_digest(cli: &FakeConfig, target: &HermesTarget) -> String { cli.snapshot(target, &[]).unwrap().config_digest }

struct Fixture { root: PathBuf, cli: FakeConfig, store: HermesStore, target: HermesTarget }
impl Fixture {
    fn new() -> Self {
        let root = temp().join("lifecycle"); fs::create_dir_all(&root).unwrap();
        let target_home = root.join("target-home"); fs::create_dir(&target_home).unwrap();
        let executable = root.join("hermes"); fs::write(&executable, b"synthetic executable").unwrap();
        let target = HermesTarget { executable, home: target_home, display_name: "synthetic".into() };
        let cli = FakeConfig::new(BTreeMap::from([("providers.foreign".into(), json!({"base_url":"https://foreign.example.test"}))]));
        let store = HermesStore::new(root.join("private-store"));
        let key = HermesStore::target_key(&target).unwrap();
        let backup_hash = format!("{:x}", sha2::Sha256::digest(key.as_bytes()));
        *cli.backup_to_observe.borrow_mut() = Some(root.join(format!("private-store/backups/{backup_hash}.raw")));
        Self { root, cli, store, target }
    }
    fn selection(&self, model_id: &str) -> HermesSelection { let mut s = selection(ALPHA); s.target = self.target.clone(); s.model_id = model_id.into(); s }
}

#[test]
fn lifecycle_connect_is_idempotent_and_disconnect_restores_only_managed_fields() {
    let fx = Fixture::new();
    let s = fx.selection("team/sonnet");
    connect(&fx.cli, &fx.store, s.clone(), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    assert!(*fx.cli.backup_seen_before_write.borrow());
    let connected = fx.cli.all_stored();
    assert_eq!(connected["providers.foreign"], json!({"base_url":"https://foreign.example.test"}));
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::Configured);
    connect(&fx.cli, &fx.store, s, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), connected);
    disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), BTreeMap::from([("providers.foreign".into(), json!({"base_url":"https://foreign.example.test"}))]));
    assert!(!fx.cli.backup_to_observe.borrow().as_ref().unwrap().exists());
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::NotConnected);
    fs::remove_dir_all(fx.root).unwrap();
}

#[test]
fn lifecycle_failed_activation_restores_only_completed_owned_writes() {
    let fx = Fixture::new();
    fx.cli.fail_write_number(2);
    let before = fx.cli.all_stored();
    assert!(connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).is_err());
    assert_eq!(fx.cli.all_stored(), before);
    assert!(!fx.store.has_pending_operation().unwrap());
    fs::remove_dir_all(fx.root).unwrap();
}

#[test]
fn lifecycle_update_keeps_first_original_and_detects_user_drift_before_disconnect() {
    let fx = Fixture::new();
    let mut first = fx.selection("team/model-one");
    first.model_id = "team/model-one".into();
    connect(&fx.cli, &fx.store, first, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let mut second = fx.selection("team/model-two");
    second.model_id = "team/model-two".into();
    connect(&fx.cli, &fx.store, second, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::Configured);
    fx.cli.values.borrow_mut().insert("model.default".into(), json!("user-choice"));
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::Drifted);
    let drifted = fx.cli.all_stored();
    assert_eq!(disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)), Err(HermesError::Conflict));
    assert_eq!(fx.cli.all_stored(), drifted);
    fs::remove_dir_all(fx.root).unwrap();
}

#[test]
fn lifecycle_records_initial_values_even_when_some_already_match() {
    let fx = Fixture::new();
    fx.cli.values.borrow_mut().insert("model.provider".into(), json!("keyzapper"));
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), BTreeMap::from([
        ("providers.foreign".into(), json!({"base_url":"https://foreign.example.test"})),
        ("model.provider".into(), json!("keyzapper")),
    ]));
    fs::remove_dir_all(fx.root).unwrap();
}

#[test]
fn lifecycle_journal_save_failure_happens_before_first_config_write() {
    let fx = Fixture::new();
    let before = fx.cli.all_stored();
    fs::create_dir_all(&fx.root).unwrap();
    // A registry that cannot be read must block the operation before any config write.
    fs::create_dir_all(fx.root.join("private-store")).unwrap();
    fs::write(fx.root.join("private-store/registry.json"), b"not json").unwrap();
    assert!(connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).is_err());
    assert_eq!(fx.cli.all_stored(), before);
    assert_eq!(fs::read(fx.root.join("private-store/registry.json")).unwrap(), b"not json");
    assert!(!fx.cli.calls.borrow().iter().any(|call| call.starts_with("write:")));
}

#[test]
fn lifecycle_stale_confirmation_rejects_connect_and_disconnect_without_writes() {
    let fx = Fixture::new();
    let stale = "not-the-preview-digest";
    let before = fx.cli.all_stored();
    assert_eq!(connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), stale), Err(HermesError::Conflict));
    assert_eq!(fx.cli.all_stored(), before);
    assert!(!fx.store.has_pending_operation().unwrap());
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let before = fx.cli.all_stored();
    assert_eq!(disconnect(&fx.cli, &fx.store, &fx.target, stale), Err(HermesError::Conflict));
    assert_eq!(fx.cli.all_stored(), before);
}

#[test]
fn lifecycle_recovery_rolls_back_crash_after_config_write_before_progress_save() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap().unwrap();
    let plan = plan_connect(&fx.cli, fx.selection("team/model-next"), &profile(ALPHA), "/opt/keyzapper-helper", Some(&owner), &ManagedConfig::default()).unwrap();
    fx.store.save_raw_backup(&fx.target, &fx.cli.bytes()).unwrap();
    let first = plan.changes.first().unwrap().clone();
    fx.cli.write(&fx.target, &first.key, match &first.after { StoredValue::Present(v) => Some(v), StoredValue::Absent => None }).unwrap();
    fx.store.save_journal(&key, &keyzapper_core::hermes::store::Journal { plan, completed: Vec::new(), phase: keyzapper_core::hermes::store::Phase::Applying, operation: keyzapper_core::hermes::store::Operation::Connect }).unwrap();
    let digest = fx.cli.snapshot(&fx.target, &[]).unwrap().config_digest;
    recover(&fx.cli, &fx.store, &fx.target, &digest, keyzapper_core::hermes::RecoveryAction::Rollback).unwrap();
    assert!(fx.cli.all_stored().get(&first.key).map(|v| StoredValue::Present(v.clone())).unwrap_or(StoredValue::Absent) == first.before);
    assert!(fx.store.owner(&key).unwrap() == Some(owner));
}

#[test]
fn lifecycle_recovery_never_overwrites_a_third_user_value() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap().unwrap();
    let plan = plan_connect(&fx.cli, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", Some(&owner), &ManagedConfig::default()).unwrap();
    let field = plan.changes[0].key.clone();
    fx.store.save_raw_backup(&fx.target, &fx.cli.bytes()).unwrap();
    fx.store.save_journal(&key, &keyzapper_core::hermes::store::Journal { plan, completed: Vec::new(), phase: keyzapper_core::hermes::store::Phase::Applying, operation: keyzapper_core::hermes::store::Operation::Connect }).unwrap();
    fx.cli.values.borrow_mut().insert(field.clone(), json!("user-edit"));
    let digest = fx.cli.snapshot(&fx.target, &[]).unwrap().config_digest;
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, "stale-confirmation", keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::Conflict));
    assert_eq!(fx.cli.all_stored()[&field], json!("user-edit"));
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &digest, keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
    assert_eq!(fx.cli.all_stored()[&field], json!("user-edit"));
    assert!(fx.store.has_pending_operation().unwrap());
    assert!(matches!(fx.store.journal(&key).unwrap().unwrap().phase, keyzapper_core::hermes::store::Phase::RecoveryRequired));
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::RecoveryRequired);
}

#[test]
fn lifecycle_recovery_rolls_back_interrupted_disconnect_to_configured_owner() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/sonnet"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap().unwrap();
    let keys = owner.last_written.keys().cloned().collect::<Vec<_>>();
    let snapshot = fx.cli.snapshot(&fx.target, &keys).unwrap();
    let changes = keys.iter().map(|k| keyzapper_core::hermes::FieldChange { key:k.clone(), before:snapshot.fields[k].clone(), after:owner.original[k].clone() }).filter(|c| c.before != c.after).collect();
    let plan = keyzapper_core::hermes::BindingPlan { selection:owner.selection.clone(), snapshot, changes };
    fx.store.save_raw_backup(&fx.target, &fx.cli.bytes()).unwrap();
    let first = plan.changes[0].clone();
    fx.cli.write(&fx.target, &first.key, match &first.after { StoredValue::Present(v) => Some(v), StoredValue::Absent=>None }).unwrap();
    fx.store.save_journal(&key, &keyzapper_core::hermes::store::Journal { plan, completed:vec![], phase:keyzapper_core::hermes::store::Phase::Applying, operation:keyzapper_core::hermes::store::Operation::Disconnect }).unwrap();
    let digest = fx.cli.snapshot(&fx.target, &[]).unwrap().config_digest;
    recover(&fx.cli, &fx.store, &fx.target, &digest, keyzapper_core::hermes::RecoveryAction::Rollback).unwrap();
    assert!(fx.store.owner(&key).unwrap() == Some(owner));
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::Configured);
}

fn pending_update(fx: &Fixture) -> (String, keyzapper_core::hermes::Ownership) {
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap().unwrap();
    let plan = plan_connect(&fx.cli, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", Some(&owner), &ManagedConfig::default()).unwrap();
    fx.store.save_raw_backup(&fx.target, &fx.cli.raw_config(&fx.target).unwrap().unwrap()).unwrap();
    fx.store.save_journal(&key, &keyzapper_core::hermes::store::Journal { plan: plan.clone(), completed: vec![], phase: keyzapper_core::hermes::store::Phase::Applying, operation: keyzapper_core::hermes::store::Operation::Connect }).unwrap();
    for change in &plan.changes { fx.cli.write(&fx.target, &change.key, match &change.after { StoredValue::Present(v) => Some(v), StoredValue::Absent => None }).unwrap(); }
    (key, owner)
}

#[test]
fn lifecycle_recovery_rollback_write_failure_retains_original_owner_and_backup() {
    let fx = Fixture::new();
    let (key, owner) = pending_update(&fx);
    fx.cli.fail_write_number(*fx.cli.write_count.borrow() + 1);
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::RollbackFailed));
    assert!(fx.store.owner(&key).unwrap() == Some(owner));
    assert!(fx.store.has_pending_operation().unwrap());
    assert!(fx.cli.backup_to_observe.borrow().as_ref().unwrap().is_file());
    assert!(matches!(fx.store.journal(&key).unwrap().unwrap().phase, keyzapper_core::hermes::store::Phase::RecoveryRequired));
}

#[test]
fn lifecycle_concurrent_edit_to_unchanged_managed_field_is_not_committed() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|cli, _| { cli.values.borrow_mut().insert("model.provider".into(), json!("user-provider")); }));
    assert_eq!(connect(&fx.cli, &fx.store, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)), Err(HermesError::CliFailed));
    assert_eq!(fx.cli.all_stored()["model.provider"], json!("user-provider"));
    assert_eq!(fx.cli.all_stored()["model.default"], json!("team/first"));
    assert!(fx.store.owner(&key).unwrap() == owner);
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::Drifted);
}

#[test]
fn lifecycle_recovery_rejects_forged_journal_shapes_before_any_inverse_write() {
    let mut accepted = Vec::new();
    for case in ["extra-change", "duplicate-change", "before-mismatch", "unsafe-before", "wrong-home", "wrong-executable", "external-config-path", "selection-profile", "selection-model", "missing-change", "extra-snapshot", "unknown-progress", "duplicate-progress", "committed-phase", "missing-owner", "wrong-owner-target", "owner-extra-original", "owner-extra-last", "owner-last-mismatch", "owner-selection", "disconnect-forgery", "missing-model.base_url", "missing-model.default", "missing-model.provider", "missing-providers.keyzapper"] {
        let fx = Fixture::new();
        let (key, _) = pending_update(&fx);
        let registry_path = fx.root.join("private-store/registry.json");
        let mut registry: Value = serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        let journal = &mut registry["journals"][&key];
        match case {
            "extra-change" => { journal["plan"]["changes"].as_array_mut().unwrap().push(json!({"key":"providers.foreign","before":{"Present":{"base_url":"https://attacker.test"}},"after":{"Present":{"base_url":"https://foreign.example.test"}}})); }
            "duplicate-change" => { let change = journal["plan"]["changes"][0].clone(); journal["plan"]["changes"].as_array_mut().unwrap().push(change); }
            "before-mismatch" => journal["plan"]["changes"][0]["before"] = json!({"Present":"forged-original"}),
            "unsafe-before" => { journal["plan"]["changes"][0]["before"] = json!({"Present":"sk-private"}); journal["plan"]["snapshot"]["fields"]["model.default"] = json!({"Present":"sk-private"}); }
            "wrong-home" => journal["plan"]["selection"]["target"]["home"] = json!(fx.root),
            "wrong-executable" => journal["plan"]["selection"]["target"]["executable"] = json!("/bin/sh"),
            "external-config-path" => journal["plan"]["snapshot"]["config_path"] = json!(fx.root.join("elsewhere.yaml")),
            "selection-profile" => journal["plan"]["selection"]["profileId"] = json!(BETA),
            "selection-model" => journal["plan"]["selection"]["modelId"] = json!("team/forged"),
            "missing-change" => journal["plan"]["changes"] = json!([]),
            "extra-snapshot" => journal["plan"]["snapshot"]["fields"]["providers.foreign"] = json!("Absent"),
            "unknown-progress" => journal["completed"] = json!(["providers.foreign"]),
            "duplicate-progress" => journal["completed"] = json!(["model.default", "model.default"]),
            "committed-phase" => journal["phase"] = json!("Committed"),
            "missing-owner" => { registry["ownership"].as_object_mut().unwrap().remove(&key); }
            "wrong-owner-target" => registry["ownership"][&key]["selection"]["target"]["home"] = json!(fx.root),
            "owner-extra-original" => registry["ownership"][&key]["original"]["providers.foreign"] = json!("Absent"),
            "owner-extra-last" => registry["ownership"][&key]["last_written"]["providers.foreign"] = json!("Absent"),
            "owner-last-mismatch" => registry["ownership"][&key]["last_written"]["model.default"] = json!({"Present":"team/forged"}),
            "owner-selection" => registry["ownership"][&key]["selection"]["modelId"] = json!("team/forged"),
            "disconnect-forgery" => journal["operation"] = json!("Disconnect"),
            missing => { journal["plan"]["snapshot"]["fields"].as_object_mut().unwrap().remove(missing.strip_prefix("missing-").unwrap()); }
        }
        fs::write(registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
        let writes = *fx.cli.write_count.borrow();
        let before = fx.cli.all_stored();
        let result = recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback);
        if result != Err(HermesError::RecoveryRequired) || *fx.cli.write_count.borrow() != writes || fx.cli.all_stored() != before || !fx.store.has_pending_operation().unwrap() { accepted.push(case); }
    }
    assert!(accepted.is_empty(), "unsafe recovery cases accepted or mutated: {accepted:?}");
}

struct RawView<'a> { cli: &'a FakeConfig, raw: Option<Vec<u8>>, digest: Option<String> }
impl ConfigAccess for RawView<'_> {
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> keyzapper_core::hermes::HermesResult<keyzapper_core::hermes::ConfigSnapshot> {
        let mut snapshot = self.cli.snapshot(target, keys)?;
        if let Some(digest) = &self.digest { snapshot.config_digest = digest.clone(); }
        Ok(snapshot)
    }
    fn raw_config(&self, _: &HermesTarget) -> keyzapper_core::hermes::HermesResult<Option<Vec<u8>>> { Ok(self.raw.clone()) }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> keyzapper_core::hermes::HermesResult<()> { self.cli.set(target, key, value) }
    fn unset(&self, target: &HermesTarget, key: &str) -> keyzapper_core::hermes::HermesResult<()> { self.cli.unset(target, key) }
}

#[test]
fn lifecycle_confirmation_checks_actual_entire_raw_config_for_every_action() {
    for action in ["connect", "disconnect", "recover"] {
        let fx = Fixture::new();
        if action == "recover" { pending_update(&fx); }
        else if action == "disconnect" { connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap(); }
        let bytes = fx.cli.raw_config(&fx.target).unwrap().unwrap();
        let mut raw: Value = serde_json::from_slice(&bytes).unwrap();
        raw["unrelated-editor-setting"] = json!(true);
        let access = RawView { cli: &fx.cli, raw: Some(serde_json::to_vec(&raw).unwrap()), digest: None };
        let writes = *fx.cli.write_count.borrow();
        let result = match action {
            "connect" => connect(&access, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)),
            "disconnect" => disconnect(&access, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)),
            _ => recover(&access, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback),
        };
        assert_eq!(result, Err(HermesError::Conflict), "{action}");
        assert_eq!(*fx.cli.write_count.borrow(), writes, "{action}");
    }
}

#[test]
fn lifecycle_recovery_refuses_unchanged_managed_drift_before_inverse() {
    let fx = Fixture::new();
    let (key, owner) = pending_update(&fx);
    fx.cli.values.borrow_mut().insert("model.provider".into(), json!("user-provider"));
    let before = fx.cli.all_stored();
    let writes = *fx.cli.write_count.borrow();
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    assert_eq!(fx.cli.all_stored(), before);
    assert!(fx.store.owner(&key).unwrap() == Some(owner));
    assert!(fx.store.has_pending_operation().unwrap());
}

#[test]
fn lifecycle_missing_config_is_not_created_or_replaced_with_a_default() {
    let fx = Fixture::new();
    let empty_digest = format!("{:x}", sha2::Sha256::digest([]));
    let access = RawView { cli: &fx.cli, raw: None, digest: Some(empty_digest.clone()) };
    let before = fx.cli.all_stored();
    assert_eq!(connect(&access, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &empty_digest), Err(HermesError::InvalidConfig));
    assert_eq!(*fx.cli.write_count.borrow(), 0);
    assert_eq!(fx.cli.all_stored(), before);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap().is_none());
}

#[test]
fn lifecycle_readback_mismatch_never_clobbers_third_value_and_keeps_recovery_files() {
    let fx = Fixture::new();
    let before = fx.cli.raw_config(&fx.target).unwrap().unwrap();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|cli, n| {
        if n == 1 { cli.values.borrow_mut().insert("model.base_url".into(), json!("https://user.example.test")); }
    }));
    assert_eq!(connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)), Err(HermesError::RollbackFailed));
    assert_eq!(*fx.cli.write_count.borrow(), 1);
    assert_eq!(fx.cli.all_stored()["model.base_url"], json!("https://user.example.test"));
    assert!(fx.store.has_pending_operation().unwrap());
    assert_eq!(fs::read(fx.cli.backup_to_observe.borrow().as_ref().unwrap()).unwrap(), before);
    assert!(fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap().is_none());
}

#[test]
fn lifecycle_deterministic_concurrent_unrelated_edit_survives_connect_and_disconnect() {
    let fx = Fixture::new();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|cli, n| {
        if n == 1 {
            cli.values.borrow_mut().insert("providers.foreign".into(), json!({"user":"concurrent-provider-edit"}));
            cli.values.borrow_mut().insert("display.theme".into(), json!("user-theme"));
        }
    }));
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), BTreeMap::from([("providers.foreign".into(), json!({"user":"concurrent-provider-edit"})), ("display.theme".into(), json!("user-theme"))]));
    assert!(!fx.store.has_pending_operation().unwrap());
}

#[test]
fn lifecycle_deterministic_managed_edit_before_its_write_is_never_overwritten() {
    let fx = Fixture::new();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|cli, n| {
        if n == 1 { cli.values.borrow_mut().insert("model.default".into(), json!("user-choice")); }
    }));
    assert_eq!(connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)), Err(HermesError::RollbackFailed));
    assert_eq!(*fx.cli.write_count.borrow(), 1);
    assert_eq!(fx.cli.all_stored()["model.default"], json!("user-choice"));
    assert!(fx.store.has_pending_operation().unwrap());
    assert!(fx.cli.backup_to_observe.borrow().as_ref().unwrap().is_file());
}

#[test]
fn lifecycle_actual_crash_between_config_write_and_progress_is_recovered_from_current() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let before = fx.cli.all_stored();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|_, _| panic!("synthetic process interruption after config write")));
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connect(&fx.cli, &fx.store, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target))));
    assert!(interrupted.is_err());
    assert_eq!(fx.cli.all_stored()["model.default"], json!("team/next"));
    assert!(fx.store.journal(&key).unwrap().unwrap().completed.is_empty());
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target).unwrap(), BindingHealth::RecoveryRequired);
    *fx.cli.after_write.borrow_mut() = None;
    recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback).unwrap();
    assert_eq!(fx.cli.all_stored(), before);
    assert!(fx.store.owner(&key).unwrap() == owner);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.cli.backup_to_observe.borrow().as_ref().unwrap().exists());
}

#[test]
fn lifecycle_moved_helper_repair_replans_explicit_canonical_path_and_keeps_originals() {
    let fx = Fixture::new();
    let old_dir = fx.root.join("old install"); let new_dir = fx.root.join("new install");
    fs::create_dir(&old_dir).unwrap(); fs::create_dir(&new_dir).unwrap();
    fs::write(old_dir.join("keyzapper-helper"), b"synthetic helper").unwrap();
    let old = fs::canonicalize(old_dir.join("keyzapper-helper")).unwrap();
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), old.to_str().unwrap(), &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let key = HermesStore::target_key(&fx.target).unwrap();
    let owner = fx.store.owner(&key).unwrap().unwrap();
    fs::rename(&old, new_dir.join("keyzapper-helper")).unwrap();
    let new = fs::canonicalize(new_dir.join("keyzapper-helper")).unwrap();
    assert_ne!(old, new);
    let writes = *fx.cli.write_count.borrow();
    let plan = plan_connect(&fx.cli, fx.selection("team/first"), &profile(ALPHA), new.to_str().unwrap(), Some(&owner), &ManagedConfig::default()).unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert_eq!(plan.changes[0].key, "providers.keyzapper");
    assert!(matches!(&plan.changes[0].after, StoredValue::Present(v) if v["key_cmd"].as_str().unwrap().contains(new.to_str().unwrap())));
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), new.to_str().unwrap(), &ManagedConfig::default(), &plan.snapshot.config_digest).unwrap();
    let repaired = fx.store.owner(&key).unwrap().unwrap();
    assert!(repaired.original == owner.original);
    assert!(repaired.last_written != owner.last_written);
    assert!(fx.cli.all_stored()["providers.keyzapper"]["key_cmd"].as_str().unwrap().contains(new.to_str().unwrap()));
    disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), BTreeMap::from([("providers.foreign".into(), json!({"base_url":"https://foreign.example.test"}))]));
}

#[test]
fn lifecycle_missing_selected_profile_mismatch_never_uses_supplied_default_profile() {
    let fx = Fixture::new();
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let before = fx.cli.all_stored();
    let writes = *fx.cli.write_count.borrow();
    assert_eq!(connect(&fx.cli, &fx.store, fx.selection("team/next"), &profile(BETA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)), Err(HermesError::PolicyDenied));
    assert_eq!(fx.cli.all_stored(), before);
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    assert!(!fx.store.has_pending_operation().unwrap());
    // Inventory-aware broken-profile status belongs to Task 6; inspect cannot infer deletion.
}

#[test]
fn lifecycle_disconnect_restores_exact_explicit_model_fields() {
    let fx = Fixture::new();
    for (key, value) in [("model.provider", "original-provider"), ("model.default", "original-model"), ("model.base_url", "https://original.example.test/api")] { fx.cli.values.borrow_mut().insert(key.into(), json!(value)); }
    let before = fx.cli.all_stored();
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    connect(&fx.cli, &fx.store, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)).unwrap();
    assert_eq!(fx.cli.all_stored(), before);
}

#[test]
fn lifecycle_invalid_owner_cannot_authorize_connect_or_disconnect_writes() {
    let mut unsafe_cases = Vec::new();
    for action in ["connect", "disconnect"] {
        for case in ["extra-owned-key", "missing-original", "invalid-original", "wrong-owner-target"] {
            let fx = Fixture::new();
            connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
            let key = HermesStore::target_key(&fx.target).unwrap();
            let mut owner = fx.store.owner(&key).unwrap().unwrap();
            match case {
                "extra-owned-key" => { owner.original.insert("model.unowned".into(), StoredValue::Present(json!("forged"))); owner.last_written.insert("model.unowned".into(), StoredValue::Absent); }
                "missing-original" => { owner.original.remove("model.default"); }
                "invalid-original" => { owner.original.insert("model.default".into(), StoredValue::Present(json!({"forged":"shape"}))); }
                _ => owner.selection.target.home = fx.root.clone(),
            }
            fx.store.save(&BTreeMap::from([(key, owner)])).unwrap();
            let before = fx.cli.all_stored();
            let writes = *fx.cli.write_count.borrow();
            let result = if action == "connect" { connect(&fx.cli, &fx.store, fx.selection("team/next"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)) }
                else { disconnect(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target)) };
            if result != Err(HermesError::Conflict) || *fx.cli.write_count.borrow() != writes || fx.cli.all_stored() != before { unsafe_cases.push(format!("{action}/{case}")); }
        }
    }
    assert!(unsafe_cases.is_empty(), "invalid owner accepted or wrote config: {unsafe_cases:?}");
}

#[test]
fn lifecycle_initial_recovery_cannot_forge_inverse_values_by_editing_snapshot_too() {
    let fx = Fixture::new();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|_, _| panic!("synthetic first-connect crash")));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)))).is_err());
    *fx.cli.after_write.borrow_mut() = None;
    let key = HermesStore::target_key(&fx.target).unwrap();
    let mut journal = fx.store.journal(&key).unwrap().unwrap();
    let forged = StoredValue::Present(json!("https://forged-original.test"));
    journal.plan.snapshot.fields.insert("model.base_url".into(), forged.clone());
    journal.plan.changes[0].before = forged;
    fx.store.save_journal(&key, &journal).unwrap();
    let before = fx.cli.all_stored();
    let writes = *fx.cli.write_count.borrow();
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
    assert_eq!(fx.cli.all_stored(), before);
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    assert!(fx.store.has_pending_operation().unwrap());
}

#[test]
fn lifecycle_target_lock_blocks_operation_until_release() {
    use std::sync::mpsc;
    use std::time::Duration;
    let fx = Fixture::new();
    let guard = fx.store.lock_target(&fx.target).unwrap();
    let target = fx.target.clone();
    let store_path = fx.root.join("private-store");
    let digest = current_digest(&fx.cli, &fx.target);
    let values = fx.cli.all_stored();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let cli = FakeConfig::new(values);
        let store = HermesStore::new(store_path);
        let mut selected = selection(ALPHA); selected.target = target;
        ready_tx.send(()).unwrap();
        let result = connect(&cli, &store, selected, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &digest);
        done_tx.send(result).unwrap();
    });
    ready_rx.recv().unwrap();
    assert!(matches!(done_rx.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
    assert!(fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap().is_none());
    drop(guard);
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
    worker.join().unwrap();
    assert!(fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap().is_some());
}

#[test]
fn store_concurrent_registry_commits_and_journals_preserve_other_owners() {
    use std::sync::{Arc, Barrier};
    let fx = Fixture::new();
    let (original_key, original_owner) = pending_update(&fx);
    let barrier = Arc::new(Barrier::new(5));
    let mut workers = Vec::new();
    let mut expected = BTreeMap::from([(original_key.clone(), original_owner.clone())]);
    for n in 0..4 {
        let mut target = fx.target.clone(); target.home = fx.root.join(format!("other-home-{n}")); fs::create_dir(&target.home).unwrap();
        let mut selected = selection(ALPHA); selected.target = target.clone(); selected.model_id = format!("team/model-{n}");
        let plan = plan_connect(&FakeConfig::new(BTreeMap::new()), selected, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
        let owner = keyzapper_core::hermes::binding::ownership_after(&plan, None);
        let key = HermesStore::target_key(&target).unwrap();
        expected.insert(key.clone(), owner.clone());
        let store_path = fx.root.join("private-store"); let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let store = HermesStore::new(store_path);
            barrier.wait();
            store.save_journal(&key, &keyzapper_core::hermes::store::Journal { plan, completed: vec![], phase: keyzapper_core::hermes::store::Phase::Prepared, operation: keyzapper_core::hermes::store::Operation::Connect }).unwrap();
            store.commit(&key, Some(owner)).unwrap();
        }));
    }
    barrier.wait();
    for worker in workers { worker.join().unwrap(); }
    assert!(fx.store.load().unwrap() == expected);
    assert!(fx.store.journal(&original_key).unwrap().is_some());
    assert!(fx.store.owner(&original_key).unwrap() == Some(original_owner));
}

fn other_executable(fx: &Fixture) -> HermesTarget {
    let mut target = fx.target.clone();
    target.executable = fx.root.join("hermes-b");
    fs::write(&target.executable, b"different real binary").unwrap();
    target
}

#[test]
fn lifecycle_shared_resource_pending_connect_blocks_other_executable() {
    let fx = Fixture::new();
    let other = other_executable(&fx);
    let before = fx.cli.all_stored();
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|_, _| panic!("interrupt A after first config write")));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)))).is_err());
    *fx.cli.after_write.borrow_mut() = None;
    assert!(!fx.cli.all_stored().contains_key("providers.keyzapper"));
    let mut selected = fx.selection("team/first"); selected.target = other.clone();
    let writes = *fx.cli.write_count.borrow();
    assert_eq!(inspect(&fx.cli, &fx.store, &other), Ok(BindingHealth::RecoveryRequired));
    assert_eq!(connect(&fx.cli, &fx.store, selected, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &other)), Err(HermesError::RecoveryRequired));
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback).unwrap();
    assert_eq!(fx.cli.all_stored(), before);
    assert!(fx.store.owner(&HermesStore::target_key(&other).unwrap()).unwrap().is_none());
}

#[test]
fn lifecycle_shared_resource_committed_owner_prevents_silent_adoption() {
    let fx = Fixture::new();
    let other = other_executable(&fx);
    connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)).unwrap();
    let owner = fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap();
    // Even if the provider was manually removed, another binary must not adopt A's config.
    fx.cli.values.borrow_mut().remove("providers.keyzapper");
    let before = fx.cli.all_stored();
    let writes = *fx.cli.write_count.borrow();
    let mut selected = fx.selection("team/first"); selected.target = other.clone();
    assert_eq!(connect(&fx.cli, &fx.store, selected, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &other)), Err(HermesError::Conflict));
    assert_eq!(fx.cli.all_stored(), before);
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    assert!(fx.store.owner(&HermesStore::target_key(&fx.target).unwrap()).unwrap() == owner);
}

#[test]
fn lifecycle_shared_resource_legacy_overlap_recovery_cannot_clobber_completed_owner() {
    let fx = Fixture::new();
    let other = other_executable(&fx);
    *fx.cli.after_write.borrow_mut() = Some(Box::new(|_, _| panic!("interrupt A")));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connect(&fx.cli, &fx.store, fx.selection("team/first"), &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &current_digest(&fx.cli, &fx.target)))).is_err());
    *fx.cli.after_write.borrow_mut() = None;
    let mut selected = fx.selection("team/first"); selected.target = other.clone();
    // Reproduce a pre-fix registry: B completed while A was pending, on the same map.
    let plan = plan_connect(&fx.cli, selected, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    for change in &plan.changes { fx.cli.write(&other, &change.key, match &change.after { StoredValue::Present(v) => Some(v), StoredValue::Absent => None }).unwrap(); }
    let owner = keyzapper_core::hermes::binding::ownership_after(&plan, None);
    let key_b = HermesStore::target_key(&other).unwrap();
    fx.store.save(&BTreeMap::from([(key_b.clone(), owner.clone())])).unwrap();
    let completed = fx.cli.all_stored();
    let writes = *fx.cli.write_count.borrow();
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &current_digest(&fx.cli, &fx.target), keyzapper_core::hermes::RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
    assert_eq!(fx.cli.all_stored(), completed);
    assert_eq!(*fx.cli.write_count.borrow(), writes);
    assert!(fx.store.owner(&key_b).unwrap() == Some(owner));
    assert!(fx.store.has_pending_operation().unwrap());
}

#[cfg(unix)]
#[test]
fn lifecycle_shared_resource_lock_covers_other_executable_and_home_alias() {
    use std::{sync::mpsc, time::Duration};
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    let mut other = other_executable(&fx);
    symlink(&fx.target.home, fx.root.join("home-alias")).unwrap();
    other.home = fx.root.join("home-alias");
    // File-backed config is genuinely shared, rather than independent fake maps.
    fs::write(fx.target.home.join("config.yaml"), fx.cli.bytes()).unwrap();
    struct FileConfig;
    impl FileConfig {
        fn read(&self, target: &HermesTarget) -> FakeConfig {
            let raw: Value = serde_json::from_slice(&fs::read(target.home.join("config.yaml")).unwrap()).unwrap();
            let mut fields = BTreeMap::new();
            for (section, entries) in raw.as_object().unwrap() { for (key, value) in entries.as_object().unwrap() { fields.insert(format!("{section}.{key}"), value.clone()); } }
            FakeConfig::new(fields)
        }
    }
    impl ConfigAccess for FileConfig {
        fn snapshot(&self, t: &HermesTarget, k: &[String]) -> keyzapper_core::hermes::HermesResult<keyzapper_core::hermes::ConfigSnapshot> { self.read(t).snapshot(t, k) }
        fn raw_config(&self, t: &HermesTarget) -> keyzapper_core::hermes::HermesResult<Option<Vec<u8>>> { Ok(Some(fs::read(t.home.join("config.yaml")).unwrap())) }
        fn set(&self, t: &HermesTarget, k: &str, v: &Value) -> keyzapper_core::hermes::HermesResult<()> { let cli = self.read(t); cli.set(t, k, v)?; fs::write(t.home.join("config.yaml"), cli.bytes()).unwrap(); Ok(()) }
        fn unset(&self, t: &HermesTarget, k: &str) -> keyzapper_core::hermes::HermesResult<()> { let cli = self.read(t); cli.unset(t, k)?; fs::write(t.home.join("config.yaml"), cli.bytes()).unwrap(); Ok(()) }
    }
    let guard = fx.store.lock_target(&fx.target).unwrap();
    let store_path = fx.root.join("private-store");
    let (ready_tx, ready_rx) = mpsc::channel(); let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut selected = selection(ALPHA); selected.target = other.clone();
        let digest = FileConfig.snapshot(&other, &[]).unwrap().config_digest;
        ready_tx.send(()).unwrap();
        done_tx.send(connect(&FileConfig, &HermesStore::new(store_path), selected, &profile(ALPHA), "/opt/keyzapper-helper", &ManagedConfig::default(), &digest)).unwrap();
    });
    ready_rx.recv().unwrap();
    let blocked = matches!(done_rx.recv_timeout(Duration::from_millis(100)), Err(mpsc::RecvTimeoutError::Timeout));
    let unchanged = FileConfig.read(&fx.target).all_stored() == fx.cli.all_stored();
    drop(guard);
    if blocked { assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(())); }
    worker.join().unwrap();
    assert!(blocked, "same-home B bypassed A's resource lock");
    assert!(unchanged);
    assert_eq!(FileConfig.read(&fx.target).all_stored()["model.provider"], json!("keyzapper"));
}

#[test]
fn binding_quotes_helper_and_never_embeds_key_material() {
    let p = profile(ALPHA);
    let value = desired_provider(&p, "/Applications/AI'Switch.app/Contents/MacOS/keyzapper-helper").unwrap();
    assert_eq!(value["api_mode"], "anthropic_messages");
    assert_eq!(value["base_url"], "https://gw.example.test");
    assert_eq!(value["key_cmd"], "'/Applications/AI'\\''Switch.app/Contents/MacOS/keyzapper-helper' credential --profile 123E4567-E89B-12D3-A456-426614174000 --json");
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert!(value.get("api_key").is_none());
    let mut sso = profile(BETA); sso.auth_type = AuthType::Oidc;
    assert_eq!(desired_provider(&sso, "/opt/keyzapper-helper").unwrap()["base_url"], "https://gw.example.test");
}

#[test]
fn binding_accepts_safe_existing_base_url_and_rejects_unsafe_inverse_values() {
    let existing = FakeConfig::new(BTreeMap::from([("model.base_url".into(), json!("https://old.example.test/api"))]));
    let plan = plan_connect(&existing, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    assert!(matches!(plan.changes.iter().find(|c| c.key == "model.base_url").unwrap().before, StoredValue::Present(ref value) if value == &json!("https://old.example.test/api")));
    for unsafe_url in ["https://user:pass@old.example.test", "https://old.example.test/?token=x", "https://old.example.test/#frag"] {
        let access = FakeConfig::new(BTreeMap::from([("model.base_url".into(), json!(unsafe_url))]));
        let result = plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default());
        assert!(matches!(result, Err(HermesError::InvalidConfig)));
        assert!(!access.calls.borrow().iter().any(|call| call.starts_with("write:")));
    }
}

#[test]
fn binding_update_preserves_preexisting_original_and_advances_last_written() {
    let initial_access = FakeConfig::new(BTreeMap::from([("model.default".into(), json!("original-model"))]));
    let mut first_selection = selection(ALPHA);
    first_selection.model_id = "team/model-one".into();
    let first_plan = plan_connect(&initial_access, first_selection, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    let first_owner = keyzapper_core::hermes::binding::ownership_after(&first_plan, None);
    assert!(matches!(first_owner.original["model.default"], StoredValue::Present(ref v) if v == &json!("original-model")));
    assert!(matches!(first_owner.last_written["model.default"], StoredValue::Present(ref v) if v == &json!("team/model-one")));

    let mut update_values = first_owner.last_written.iter().filter_map(|(key, value)| match value { StoredValue::Present(v) => Some((key.clone(), v.clone())), StoredValue::Absent => None }).collect::<BTreeMap<_, _>>();
    let update_access = FakeConfig::new(std::mem::take(&mut update_values));
    let mut second_selection = selection(ALPHA);
    second_selection.model_id = "team/model-two".into();
    let update = plan_connect(&update_access, second_selection, &profile(ALPHA), "/opt/keyzapper-helper", Some(&first_owner), &ManagedConfig::default()).unwrap();
    let model_change = update.changes.iter().find(|change| change.key == "model.default").unwrap();
    assert!(matches!(model_change.before, StoredValue::Present(ref value) if value == &json!("team/model-one")));
    assert!(matches!(model_change.after, StoredValue::Present(ref value) if value == &json!("team/model-two")));
    let updated = keyzapper_core::hermes::binding::ownership_after(&update, Some(&first_owner));
    assert!(matches!(updated.original["model.default"], StoredValue::Present(ref v) if v == &json!("original-model")));
    assert!(matches!(updated.last_written["model.default"], StoredValue::Present(ref v) if v == &json!("team/model-two")));
    assert!(update_access.calls.borrow().iter().all(|call| !call.starts_with("write:")));
}

#[test]
fn binding_rejects_incomplete_or_different_target_ownership_before_snapshot() {
    let access = FakeConfig::new(BTreeMap::new());
    let initial = plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    let first_owner = keyzapper_core::hermes::binding::ownership_after(&initial, None);
    let mut incomplete = first_owner.clone(); incomplete.last_written.remove("model.default");
    assert!(matches!(plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", Some(&incomplete), &ManagedConfig::default()), Err(HermesError::Conflict)));
    let mut other_home = first_owner.clone(); other_home.selection.target.home = "/other/home".into();
    assert!(matches!(plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", Some(&other_home), &ManagedConfig::default()), Err(HermesError::Conflict)));
    let mut other_executable = first_owner.clone(); other_executable.selection.target.executable = "/other/hermes".into();
    assert!(matches!(plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", Some(&other_executable), &ManagedConfig::default()), Err(HermesError::Conflict)));
    assert_eq!(access.calls.borrow().len(), 1, "invalid owners must be rejected before another snapshot");
}

#[test]
fn binding_policy_denial_happens_before_any_write() {
    let access = FakeConfig::new(BTreeMap::new());
    let policy = ManagedConfig::from_values(&serde_json::from_value(json!({"AllowedGatewayHosts":["allowed.example"]})).unwrap());
    let result = plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &policy);
    assert!(matches!(result, Err(HermesError::PolicyDenied)));
    assert!(!access.calls.borrow().iter().any(|call| call.starts_with("write:")));
    assert!(access.calls.borrow().is_empty());
}

#[test]
fn binding_plan_is_idempotent_and_rejects_unowned_provider_collision() {
    let access = FakeConfig::new(BTreeMap::new());
    let plan = plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    assert_eq!(plan.changes.len(), 4);
    let owned = keyzapper_core::hermes::binding::ownership_after(&plan, None);
    for change in &plan.changes { if let StoredValue::Present(value) = &change.after { access.values.borrow_mut().insert(change.key.clone(), value.clone()); } }
    let duplicate = plan_connect(&access, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", Some(&owned), &ManagedConfig::default()).unwrap();
    assert!(duplicate.changes.is_empty());
    let collision = FakeConfig::new(BTreeMap::from([("providers.keyzapper".into(), json!({"base_url":"https://other.test","api_mode":"anthropic_messages","key_cmd":"/secret"}))]));
    assert!(matches!(plan_connect(&collision, selection(ALPHA), &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).err(), Some(HermesError::Collision)));
    assert_eq!(collision.calls.borrow().len(), 1);
}

#[cfg(unix)]
#[test]
fn store_refuses_registry_symlinks_and_keeps_raw_backup_out_of_exports() {
    use std::os::unix::fs::symlink;
    let root = temp(); let victim = root.join("victim.json"); fs::write(&victim, b"{}").unwrap();
    symlink(&victim, root.join("registry.json")).unwrap();
    assert!(matches!(HermesStore::new(root.clone()).load(), Err(HermesError::JournalFailed)));
    let target_home = root.join("real-home"); fs::create_dir(&target_home).unwrap();
    let executable = root.join("hermes"); fs::write(&executable, b"fake").unwrap();
    let target = HermesTarget { executable, home: target_home, display_name: "fake".into() };
    let store_dir = root.join("store"); let store = HermesStore::new(store_dir.clone());
    let raw = b"private-hermes-config-secret";
    store.save_raw_backup(&target, raw).unwrap();
    let backup_dir = store_dir.join("backups");
    let files = fs::read_dir(&backup_dir).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    assert_eq!(fs::read(&files[0]).unwrap(), raw);
    #[cfg(unix)] assert_eq!(fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777, 0o600);
    let payload = keyzapper_core::backup::BackupPayload { created_at: "test".into(), app_version: None, state: keyzapper_core::models::AppState::default(), keys: BTreeMap::new() };
    let payload = serde_json::to_vec(&payload).unwrap();
    assert!(!payload.windows(raw.len()).any(|window| window == raw));
    let missing = HermesTarget { executable: root.join("missing-exe"), home: target.home.clone(), display_name: "bad".into() };
    assert!(matches!(HermesStore::target_key(&missing), Err(HermesError::WrongTarget)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn store_defaults_and_refuses_corrupt_registry() {
    let root = temp(); let store = HermesStore::new(root.clone());
    assert!(store.load().is_ok());
    store.save(&BTreeMap::new()).unwrap();
    let registry = root.join("registry.json");
    fs::write(&registry, b"not-json").unwrap();
    assert!(matches!(store.load().err(), Some(HermesError::JournalFailed)));
    #[cfg(unix)] {
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&registry).unwrap().permissions().mode() & 0o777, 0o600);
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn store_registry_isolates_owners_by_canonical_home_and_executable() {
    use std::os::unix::fs::symlink;
    let root = temp();
    let home_a = root.join("home-a"); let home_b = root.join("home-b");
    fs::create_dir(&home_a).unwrap(); fs::create_dir(&home_b).unwrap();
    let exe_a = root.join("hermes-a"); let exe_b = root.join("hermes-b");
    fs::write(&exe_a, b"a").unwrap(); fs::write(&exe_b, b"b").unwrap();
    let target_a = HermesTarget { executable: exe_a.clone(), home: home_a.clone(), display_name: "A".into() };
    let target_b = HermesTarget { executable: exe_b.clone(), home: home_b.clone(), display_name: "B".into() };
    let mut selected_a = selection(ALPHA); selected_a.target = target_a.clone(); selected_a.model_id = "owner-a".into(); selected_a.tenant = None;
    let mut selected_b = selection(BETA); selected_b.target = target_b.clone(); selected_b.model_id = "owner-b".into(); selected_b.tenant = None;
    let plan_a = plan_connect(&FakeConfig::new(BTreeMap::new()), selected_a, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    let plan_b = plan_connect(&FakeConfig::new(BTreeMap::new()), selected_b, &profile(BETA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    let owner_a = keyzapper_core::hermes::binding::ownership_after(&plan_a, None);
    let owner_b = keyzapper_core::hermes::binding::ownership_after(&plan_b, None);

    let key_a = HermesStore::target_key(&target_a).unwrap();
    let key_b = HermesStore::target_key(&target_b).unwrap();
    let same_home_other_exe = HermesTarget { executable: exe_b.clone(), home: home_a.clone(), display_name: "A with B exe".into() };
    let key_same_home_other_exe = HermesStore::target_key(&same_home_other_exe).unwrap();
    assert_ne!(key_a, key_b);
    assert_ne!(key_a, key_same_home_other_exe);
    let registry = BTreeMap::from([(key_a.clone(), owner_a), (key_b.clone(), owner_b)]);
    let store = HermesStore::new(root.join("store"));
    store.save(&registry).unwrap();
    let reloaded = store.load().unwrap();
    assert_eq!(reloaded.len(), 2);
    assert!(matches!(reloaded[&key_a].last_written["model.default"], StoredValue::Present(ref v) if v == &json!("owner-a")));
    assert!(matches!(reloaded[&key_b].last_written["model.default"], StoredValue::Present(ref v) if v == &json!("owner-b")));

    symlink(&home_a, root.join("home-a-alias")).unwrap();
    symlink(&exe_a, root.join("hermes-a-alias")).unwrap();
    let alias_target = HermesTarget { executable: root.join("hermes-a-alias"), home: root.join("home-a-alias"), display_name: "A alias".into() };
    assert_eq!(HermesStore::target_key(&alias_target).unwrap(), key_a);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn writes_are_blocked_without_validated_compatibility_evidence() {
    let root = temp(); let target = fake_cli(&root, "echo invoked >> \"$HERMES_HOME/writes.log\"");
    fs::create_dir_all(&target.home).unwrap();
    let cli = HermesCli::new();
    assert_eq!(cli.set(&target, "model.default", &json!("model-x")).unwrap_err(), HermesError::Unsupported);
    assert_eq!(cli.unset(&target, "model.default").unwrap_err(), HermesError::Unsupported);
    assert!(!target.home.join("writes.log").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn plan_connect_rejects_a_model_that_does_not_match_the_explicit_tenant() {
    let cli = FakeConfig::new(BTreeMap::new());
    for model in ["other/sonnet", "sonnet", "team", "team/", "team-other/sonnet"] {
        let mut chosen = selection(ALPHA);
        chosen.model_id = model.into();
        assert_eq!(plan_connect(&cli, chosen, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).err(),
            Some(HermesError::PolicyDenied), "{model} must not pass the tenant guard");
    }
    assert!(cli.calls.borrow().is_empty(), "a rejected selection never even reads the config");
    let mut chosen = selection(ALPHA);
    chosen.model_id = "team/sonnet".into();
    let plan = plan_connect(&cli, chosen, &profile(ALPHA), "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
    assert_eq!(plan.selection.model_id, "team/sonnet");
    assert!(plan.changes.iter().any(|change| change.key == "model.default"
        && matches!(&change.after, StoredValue::Present(value) if value == &json!("team/sonnet"))));
}
