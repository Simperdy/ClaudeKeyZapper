//! Store failures are scheduled only in unit-test builds, never through a public API.
use super::*;
use super::store::StoreWrite;
use std::{cell::RefCell, fs};
use serde_json::json;

const PROFILE: &str = "123E4567-E89B-12D3-A456-426614174000";
struct Fake {
    fields: RefCell<BTreeMap<String, Value>>,
    /// False once the config path no longer holds a readable file, as the real CLI would fail.
    snapshot_ok: std::cell::Cell<bool>,
    writes: RefCell<usize>,
    fail_write: RefCell<Option<usize>>,
    /// The confined config path the transport maintains, as the real CLI does.
    file: RefCell<Option<PathBuf>>,
    /// Set when a whole-file restore reached this transport.
    restored: std::cell::Cell<bool>,
    /// Overrides the raw bytes this fake reports as the file's current content.
    raw_report: RefCell<Option<Vec<u8>>>,
    /// Refuses a restore whose CAS expectation or bytes this fake's store would not produce.
    last_restore: RefCell<Option<Vec<u8>>>,
}
impl Fake {
    fn bytes(&self) -> Vec<u8> {
        // A restore pinned different bytes: the raw config is now exactly what was restored.
        if self.restored.get() { return self.last_restore.borrow().clone().unwrap_or_default(); }
        serde_json::to_vec(&*self.fields.borrow()).unwrap()
    }
    fn digest(&self) -> String { format!("{:x}", sha2::Sha256::digest(self.bytes())) }
    fn write(&self, key: &str, value: Option<&Value>) -> HermesResult<()> {
        *self.writes.borrow_mut() += 1;
        if *self.fail_write.borrow() == Some(*self.writes.borrow()) { return Err(HermesError::CliFailed); }
        if let Some(value) = value { self.fields.borrow_mut().insert(key.into(), value.clone()); }
        else { self.fields.borrow_mut().remove(key); }
        self.persist();
        Ok(())
    }
    fn persist(&self) { if let Some(path) = self.file.borrow().as_ref() { fs::write(path, self.bytes()).unwrap(); } }
}
impl ConfigAccess for Fake {
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
        if !self.snapshot_ok.get() { return Err(HermesError::CliFailed); }
        Ok(ConfigSnapshot { config_path: target.home.join("config.yaml"), config_digest: format!("{:x}", sha2::Sha256::digest(self.raw_config(target)?.unwrap_or_default())), fields: keys.iter().map(|key| (key.clone(), self.fields.borrow().get(key).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent))).collect() })
    }
    fn raw_config(&self, _: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { Ok(Some(self.raw_report.borrow().clone().unwrap_or_else(|| self.bytes()))) }
    fn set(&self, _: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.write(key, Some(value)) }
    fn unset(&self, _: &HermesTarget, key: &str) -> HermesResult<()> { self.write(key, None) }
    fn restore_raw_config(&self, _target: &HermesTarget, raw: &[u8], expected: &str) -> HermesResult<()> {
        if format!("{:x}", sha2::Sha256::digest(self.raw_config(_target)?.unwrap_or_default())) != expected { return Err(HermesError::Conflict); }
        // Same compare-and-swap shape as the CLI transport: validate, replace, update the pin.
        if !serde_yaml_ng::from_slice::<Value>(raw).map_err(|_| HermesError::InvalidConfig)?.is_object() { return Err(HermesError::InvalidConfig); }
        *self.last_restore.borrow_mut() = Some(raw.to_vec());
        self.restored.set(true);
        self.fields.borrow_mut().clear();
        Ok(())
    }
}
struct Fixture { root: PathBuf, target: HermesTarget, store: store::HermesStore, cli: Fake }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hermes-faults-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join("hermes"), b"synthetic").unwrap();
        let target = HermesTarget { executable: root.join("hermes"), home: root.join("home"), display_name: "fake".into() };
        let fixture = Self { store: store::HermesStore::new(root.join("store")), root, target, cli: Fake { snapshot_ok: std::cell::Cell::new(true), fields: RefCell::new(BTreeMap::from([("providers.foreign".into(), json!({"unchanged":true})), ("model.default".into(), json!("original-model"))])), writes: RefCell::new(0), fail_write: RefCell::new(None), file: RefCell::new(None), restored: Default::default(), raw_report: RefCell::new(None), last_restore: RefCell::new(None) } };
        *fixture.cli.file.borrow_mut() = Some(fixture.target.home.join("config.yaml"));
        fixture.cli.persist();
        fixture
    }
    fn connect(&self, model: &str) -> HermesResult<()> {
        let selection = HermesSelection { target: self.target.clone(), profile_id: PROFILE.into(), model_id: model.into(), tenant: None };
        let profile = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), model.into());
        connect(&self.cli, &self.store, selection, &profile, self.root.join("keyzapper-helper").to_str().unwrap(), &ManagedConfig::default(), &self.cli.digest())
    }
    fn key(&self) -> String { store::HermesStore::target_key(&self.target).unwrap() }
    fn backup_exists(&self) -> bool { fs::read_dir(self.root.join("store/backups")).is_ok_and(|mut entries| entries.next().is_some()) }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

#[test]
fn lifecycle_connect_preview_plan_does_not_write_config_or_store() {
    let fx = Fixture::new();
    let selection = HermesSelection { target: fx.target.clone(), profile_id: PROFILE.into(), model_id: "team/preview".into(), tenant: None };
    let profile = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/preview".into());
    let before = fx.cli.fields.borrow().clone();
    let plan = plan_connect(&fx.cli, selection, &profile, fx.root.join("keyzapper-helper").to_str().unwrap(), None, &ManagedConfig::default()).unwrap();
    assert!(!plan.changes.is_empty());
    assert_eq!(*fx.cli.writes.borrow(), 0);
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert!(fx.store.owner(&fx.key()).unwrap().is_none());
    assert!(fx.store.journal(&fx.key()).unwrap().is_none());
}

#[test]
fn lifecycle_store_save_failures_before_writes_leave_config_untouched() {
    for (failure, occurrence) in [(StoreWrite::RawBackup, 1), (StoreWrite::BackupTemporary, 1), (StoreWrite::BackupSaved, 1), (StoreWrite::Journal, 1), (StoreWrite::Journal, 2)] {
        let fx = Fixture::new();
        let before = fx.cli.fields.borrow().clone();
        fx.store.fail_on(failure, occurrence);
        assert_eq!(fx.connect("team/first"), Err(HermesError::JournalFailed));
        assert_eq!(*fx.cli.writes.borrow(), 0);
        assert_eq!(*fx.cli.fields.borrow(), before);
        assert!(!fx.store.has_pending_operation().unwrap());
        assert!(!fx.backup_exists());
    }
}

#[test]
fn lifecycle_store_connect_commit_failure_rolls_back_and_preserves_original_owner() {
    let fx = Fixture::new();
    fx.connect("team/first").unwrap();
    let owner = fx.store.owner(&fx.key()).unwrap();
    let before = fx.cli.fields.borrow().clone();
    let writes = *fx.cli.writes.borrow();
    fx.store.fail_on(StoreWrite::Commit, 1);
    assert_eq!(fx.connect("team/next"), Err(HermesError::JournalFailed));
    assert!(*fx.cli.writes.borrow() > writes);
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert!(fx.store.owner(&fx.key()).unwrap() == owner);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_store_disconnect_commit_failure_rolls_back_and_preserves_original_owner() {
    let fx = Fixture::new();
    fx.connect("team/first").unwrap();
    let owner = fx.store.owner(&fx.key()).unwrap();
    let before = fx.cli.fields.borrow().clone();
    let writes = *fx.cli.writes.borrow();
    fx.store.fail_on(StoreWrite::Commit, 1);
    assert_eq!(disconnect(&fx.cli, &fx.store, &fx.target, &fx.cli.digest()), Err(HermesError::JournalFailed));
    assert!(*fx.cli.writes.borrow() > writes);
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert!(fx.store.owner(&fx.key()).unwrap() == owner);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_store_commit_failure_with_rollback_write_failure_retains_recovery() {
    for disconnecting in [false, true] {
        let fx = Fixture::new();
        fx.connect("team/first").unwrap();
        let owner = fx.store.owner(&fx.key()).unwrap();
        fx.store.fail_on(StoreWrite::Commit, 1);
        // Update changes one field; disconnect changes all four.
        *fx.cli.fail_write.borrow_mut() = Some(*fx.cli.writes.borrow() + if disconnecting { 5 } else { 2 });
        let result = if disconnecting { disconnect(&fx.cli, &fx.store, &fx.target, &fx.cli.digest()) } else { fx.connect("team/next") };
        assert_eq!(result, Err(HermesError::RollbackFailed));
        assert!(fx.store.owner(&fx.key()).unwrap() == owner);
        assert!(fx.backup_exists());
        assert_eq!(inspect(&fx.cli, &fx.store, &fx.target), Ok(BindingHealth::RecoveryRequired));
        assert!(matches!(fx.store.journal(&fx.key()).unwrap().unwrap().phase, Phase::RecoveryRequired));
    }
}

#[test]
fn lifecycle_store_progress_save_failure_rolls_back_actual_written_value() {
    let fx = Fixture::new();
    let before = fx.cli.fields.borrow().clone();
    fx.store.fail_on(StoreWrite::Journal, 4); // PreparingBackup, Prepared, Applying, first progress.
    assert_eq!(fx.connect("team/first"), Err(HermesError::JournalFailed));
    assert_eq!(*fx.cli.writes.borrow(), 2); // One forward and its inverse.
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_artifacts_preparation_interruption_is_durable_before_and_after_backup() {
    for (point, disconnecting) in [(StoreWrite::RawBackup, false), (StoreWrite::BackupSaved, false), (StoreWrite::RawBackup, true), (StoreWrite::BackupSaved, true)] {
        let fx = Fixture::new();
        if disconnecting { fx.connect("team/first").unwrap(); }
        let owner = fx.store.owner(&fx.key()).unwrap();
        let writes = *fx.cli.writes.borrow();
        let before = fx.cli.fields.borrow().clone();
        fx.store.interrupt_on(point, 1);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if disconnecting { disconnect(&fx.cli, &fx.store, &fx.target, &fx.cli.digest()) } else { fx.connect("team/first") }
        })).is_err());
        let reopened = store::HermesStore::new(fx.root.join("store"));
        assert!(reopened.journal(&fx.key()).unwrap().is_some(), "raw artifact window has no durable intent");
        let record = serde_json::to_value(reopened.journal(&fx.key()).unwrap().unwrap()).unwrap();
        assert_eq!(record["phase"], json!("PreparingBackup"));
        assert_eq!(*fx.cli.writes.borrow(), writes);
        assert_eq!(*fx.cli.fields.borrow(), before);
        let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
        assert_eq!(inspect(&fx.cli, &reopened, &fx.target), Ok(BindingHealth::RecoveryRequired));
        assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
        // Preparation never acquired inverse authority; even later edits survive cleanup.
        fx.cli.fields.borrow_mut().insert("model.default".into(), json!("later-user-edit"));
        let current = fx.cli.fields.borrow().clone();
        assert_eq!(recover(&fx.cli, &reopened, &fx.target, "stale", RecoveryAction::Rollback), Err(HermesError::Conflict));
        recover(&fx.cli, &reopened, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
        assert_eq!(*fx.cli.fields.borrow(), current);
        assert_eq!(*fx.cli.writes.borrow(), writes);
        assert!(reopened.owner(&fx.key()).unwrap() == owner);
        assert!(!reopened.has_pending_operation().unwrap());
        assert!(!fx.backup_exists());
    }
}

#[test]
fn lifecycle_restore_from_backup_after_third_party_edit_of_an_owned_key() {
    // `Rollback` refuses (RollbackFailed) once someone else changed an owned key we wrote;
    // `RestoreFromBackup` is the explicit path that replaces the file with the confirmed bytes.
    let fx = Fixture::new();
    let selection = HermesSelection { target: fx.target.clone(), profile_id: PROFILE.into(), model_id: "team/first".into(), tenant: None };
    let profile = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into());
    let mut plan = plan_connect(&fx.cli, selection, &profile, fx.root.join("keyzapper-helper").to_str().unwrap(), None, &ManagedConfig::default()).unwrap();
    // Interrupted midway: the plan is pinned to the flat JSON the fake reports (production spells
    // this YAML), so the pending file is exactly the bytes the user confirmed.
    let document: BTreeMap<String, Value> = serde_json::from_slice(&fx.cli.bytes()).unwrap();
    for key in recovery::OWNED_KEYS {
        let (section, field) = key.split_once('.').unwrap();
        let stored = document.get(section).and_then(|value| value.get(field)).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent);
        plan.snapshot.fields.insert(key.to_string(), stored.clone());
        if let Some(change) = plan.changes.iter_mut().find(|change| change.key == key) { change.before = stored; }
    }
    plan.snapshot.config_digest = format!("{:x}", sha2::Sha256::digest(fx.cli.bytes()));
    fx.store.save_raw_backup(&fx.target, &fx.cli.bytes()).unwrap();
    fx.store.save_journal(&fx.key(), &store::Journal { plan, completed: Vec::new(), phase: Phase::Applying, operation: store::Operation::Connect }).unwrap();
    // A third party then edits an owned key the interrupted operation was going to write.
    let edited = "model.default";
    fx.cli.write(edited, Some(&json!("third-party-value"))).unwrap();
    let writes = *fx.cli.writes.borrow();
    // The rollback validation refuses before any inverse write: the edit is neither the before
    // nor the after value of an owned key the interrupted operation wrote.
    let pending = fx.cli.digest();
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &pending, RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
    // Restore is bound to the bytes the user confirmed, not to the edit.
    let confirmed = fx.store.journal(&fx.key()).unwrap().unwrap().plan.snapshot.config_digest;
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, "0".repeat(64).as_str(), RecoveryAction::RestoreFromBackup), Err(HermesError::Conflict));
    assert!(!fx.cli.restored.get(), "a mismatched confirmation never reaches the transport");
    assert_eq!(*fx.cli.writes.borrow(), writes, "a refused restore emits no write");
    // A real transport only ever reports the digest of the bytes it holds, so the fake reports the
    // backup re-spelled without whitespace: the same YAML document, so the same confirmation.
    let current: BTreeMap<String, Value> = serde_yaml_ng::from_slice(&fx.store.raw_backup(&fx.target).unwrap()).unwrap();
    *fx.cli.raw_report.borrow_mut() = Some(serde_json::to_vec(&current).unwrap());
    recover(&fx.cli, &fx.store, &fx.target, &confirmed, RecoveryAction::RestoreFromBackup).unwrap();
    assert!(fx.cli.restored.get(), "the validated backup replaced the whole file");
    assert!(fx.store.owner(&fx.key()).unwrap().is_none(), "an interrupted first connect leaves no owner");
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_restore_from_backup_needs_the_exact_confirmed_digest_and_the_matching_backup() {
    let fx = Fixture::new();
    fx.store.interrupt_on(StoreWrite::Cleanup, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err());
    let writes = *fx.cli.writes.borrow();
    let before = fx.cli.fields.borrow().clone();
    // The lifecycle fake speaks flat JSON; production raw backups are YAML. Pin the journal to
    // the exact YAML form of the same document, parsed into its fields, so the real recovery
    // validation runs against the confirmable document rather than the fake's spelling.
    let yaml = b"model:\n  provider: keyzapper\n  default: team/first\n  base_url: https://gateway.test\nproviders:\n  foreign:\n    unchanged: true\n";
    let mut journal = fx.store.journal(&fx.key()).unwrap().unwrap();
    journal.plan.snapshot.fields = ["model.provider", "model.default", "model.base_url"].map(|key| {
        let (section, field) = key.split_once('.').unwrap();
        let value = serde_yaml_ng::from_slice::<BTreeMap<String, serde_json::Map<String, Value>>>(yaml).unwrap()[section][field].clone();
        (key.to_string(), StoredValue::Present(value))
    }).into_iter().chain([("providers.keyzapper".to_string(), StoredValue::Absent)]).collect();
    journal.plan.snapshot.config_digest = format!("{:x}", sha2::Sha256::digest(yaml));
    fx.store.save_raw_backup(&fx.target, yaml).unwrap();
    fx.store.save_journal(&fx.key(), &journal).unwrap();
    let store_backup = |fx: &Fixture| fs::read_dir(fx.root.join("store/backups")).unwrap().flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "raw")).map(|entry| fs::read(entry.path()).unwrap());
    assert_eq!(recovery::validate_backup(&journal, yaml), Ok(()), "the stored backup is the confirmed file");
    assert_eq!(store_backup(&fx).as_deref(), Some(&yaml[..]));
    // A backup that is no longer the confirmed bytes never authorizes a whole-file restore.
    let mut tampered = yaml.to_vec();
    tampered.extend_from_slice(b"  injected: true\n");
    assert_eq!(recovery::validate_backup(&journal, &tampered), Err(HermesError::RecoveryRequired));
    // A confirmation for different bytes is refused before anything is written.
    let other_digest = format!("{:x}", sha2::Sha256::digest(&tampered));
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &other_digest, RecoveryAction::RestoreFromBackup), Err(HermesError::Conflict));
    assert_eq!(*fx.cli.writes.borrow(), writes);
    assert!(!fx.cli.restored.get());
    assert!(matches!(fx.store.journal(&fx.key()).unwrap().unwrap().phase, Phase::CleanupPending), "a refused restore keeps the cleanup obligation");
    let _ = before;
}

/// Re-point the pending journal at `yaml` (saved as the raw backup too) and make the fake
/// report exactly that spelling as its digest, clearing every owned field it disagrees with.
#[allow(dead_code)]
fn pin_yaml(fx: &Fixture, yaml: &[u8]) -> (Vec<u8>, store::HermesStore) {
    let mut journal = fx.store.journal(&fx.key()).unwrap().unwrap();
    // `validate_backup` compares every owned field of the snapshot with the parsed backup, so the
    // snapshot travels with the bytes the test pins.
    let document = document_keys(yaml);
    for key in recovery::OWNED_KEYS {
        let stored = document.get(key).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent);
        journal.plan.snapshot.fields.insert(key.to_string(), stored);
    }
    journal.plan.snapshot.config_digest = format!("{:x}", sha2::Sha256::digest(yaml));
    let pin = store::HermesStore::new(fx.root.join("store"));
    pin.save_raw_backup(&fx.target, yaml).unwrap();
    pin.save_journal(&fx.key(), &journal).unwrap();
    *fx.cli.raw_report.borrow_mut() = Some(yaml.to_vec());
    *fx.cli.last_restore.borrow_mut() = Some(yaml.to_vec());
    (yaml.to_vec(), pin)
}

/// The confirmed document with its sections in a fixed order, so its digest is stable against
/// the fake's own unordered JSON spelling while the bytes still differ from them.
fn fixed(document: &BTreeMap<String, Value>) -> Vec<u8> {
    serde_json::to_vec(document).unwrap()
}

/// Every top-level section's fields of a YAML map as `section.field` -> value.
fn document_keys(yaml: &[u8]) -> BTreeMap<String, Value> {
    serde_yaml_ng::from_slice::<BTreeMap<String, BTreeMap<String, Value>>>(yaml).unwrap_or_default()
        .into_iter().flat_map(|(section, fields)| fields.into_iter().map(move |(field, value)| (format!("{section}.{field}"), value))).collect()
}

#[test]
fn lifecycle_restore_from_backup_refuses_a_config_path_that_is_not_a_regular_file() {
    let fx = Fixture::new();
    fx.store.interrupt_on(StoreWrite::Cleanup, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err());
    // Replace the committed config file with a directory: a non-regular file restore must refuse
    // rather than replace it.
    let path = fx.target.home.join("config.yaml");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    // The fake reports a snapshot for any target even when its file is gone or not a file.
    fx.cli.snapshot_ok.set(false);
    let (_, pin) = pin_yaml(&fx, b"model:\n  provider: keyzapper\n  default: team/first\n  base_url: https://gateway.test\n");
    let before = fx.cli.fields.borrow().clone();
    let writes = *fx.cli.writes.borrow();
    // The transport cannot read a directory, so recovery never reaches the replace step.
    let digest = pin.journal(&fx.key()).unwrap().unwrap().plan.snapshot.config_digest;
    assert!(recover(&fx.cli, &pin, &fx.target, &digest, RecoveryAction::RestoreFromBackup).is_err());
    assert!(path.is_dir(), "a non-regular config is never replaced");
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert_eq!(*fx.cli.writes.borrow(), writes);
    assert!(!fx.cli.restored.get());
    assert!(fx.backup_exists(), "the backup survives a refused restore");
}

#[test]
fn lifecycle_artifacts_post_commit_interruption_keeps_atomic_owner_and_cleanup_marker() {
    for disconnecting in [false, true] {
        let fx = Fixture::new();
        if disconnecting { fx.connect("team/first").unwrap(); }
        fx.store.interrupt_on(StoreWrite::Cleanup, 1);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if disconnecting { disconnect(&fx.cli, &fx.store, &fx.target, &fx.cli.digest()) } else { fx.connect("team/first") }
        })).is_err());
        let reopened = store::HermesStore::new(fx.root.join("store"));
        assert_eq!(reopened.owner(&fx.key()).unwrap().is_some(), !disconnecting);
        assert!(fx.backup_exists());
        assert!(reopened.journal(&fx.key()).unwrap().is_some(), "commit dropped cleanup obligation");
        let record = serde_json::to_value(reopened.journal(&fx.key()).unwrap().unwrap()).unwrap();
        assert_eq!(record["phase"], json!("CleanupPending"));
        assert_eq!(inspect(&fx.cli, &reopened, &fx.target), Ok(BindingHealth::RecoveryRequired));
        let committed = fx.cli.fields.borrow().clone();
        let owner = reopened.owner(&fx.key()).unwrap();
        let writes = *fx.cli.writes.borrow();
        assert_eq!(recover(&fx.cli, &reopened, &fx.target, "stale", RecoveryAction::Rollback), Err(HermesError::Conflict));
        assert!(fx.backup_exists());
        recover(&fx.cli, &reopened, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
        assert_eq!(*fx.cli.fields.borrow(), committed);
        assert_eq!(*fx.cli.writes.borrow(), writes, "cleanup inverted an already committed operation");
        assert!(reopened.owner(&fx.key()).unwrap() == owner);
        assert!(!reopened.has_pending_operation().unwrap());
        assert!(!fx.backup_exists());
    }
}

#[test]
fn lifecycle_artifacts_unlink_failure_retains_discoverable_cleanup() {
    let fx = Fixture::new();
    fx.store.fail_on(StoreWrite::Cleanup, 1);
    assert_eq!(fx.connect("team/first"), Err(HermesError::JournalFailed));
    assert!(fx.store.owner(&fx.key()).unwrap().is_some());
    assert!(fx.backup_exists());
    assert!(fx.store.journal(&fx.key()).unwrap().is_some(), "unlink error orphaned raw config");
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target), Ok(BindingHealth::RecoveryRequired));
    let committed = fx.cli.fields.borrow().clone();
    let writes = *fx.cli.writes.borrow();
    recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
    assert_eq!(*fx.cli.fields.borrow(), committed);
    assert_eq!(*fx.cli.writes.borrow(), writes);
    assert!(!fx.store.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_artifacts_cleanup_marker_survives_after_deletion_until_registry_finish() {
    for interrupted in [false, true] {
        let fx = Fixture::new();
        if interrupted { fx.store.interrupt_on(StoreWrite::FinishCleanup, 1); }
        else { fx.store.fail_on(StoreWrite::FinishCleanup, 1); }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first")));
        if interrupted { assert!(result.is_err()); }
        else { assert_eq!(result.unwrap(), Err(HermesError::JournalFailed)); }
        let reopened = store::HermesStore::new(fx.root.join("store"));
        let owner = reopened.owner(&fx.key()).unwrap();
        assert!(owner.is_some());
        assert!(!fx.backup_exists());
        assert!(matches!(reopened.journal(&fx.key()).unwrap().unwrap().phase, Phase::CleanupPending));
        let committed = fx.cli.fields.borrow().clone();
        let writes = *fx.cli.writes.borrow();
        assert_eq!(inspect(&fx.cli, &reopened, &fx.target), Ok(BindingHealth::RecoveryRequired));
        recover(&fx.cli, &reopened, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
        assert_eq!(*fx.cli.fields.borrow(), committed);
        assert_eq!(*fx.cli.writes.borrow(), writes);
        assert!(reopened.owner(&fx.key()).unwrap() == owner);
        assert!(!reopened.has_pending_operation().unwrap());
    }
}

#[test]
fn lifecycle_artifacts_real_unlink_error_keeps_cleanup_recoverable() {
    let fx = Fixture::new();
    fx.store.interrupt_on(StoreWrite::Cleanup, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err());
    let path = fs::read_dir(fx.root.join("store/backups")).unwrap().next().unwrap().unwrap().path();
    let retained = path.with_extension("retained");
    fs::rename(&path, &retained).unwrap(); fs::create_dir(&path).unwrap();
    let committed = fx.cli.fields.borrow().clone();
    let writes = *fx.cli.writes.borrow();
    let owner = fx.store.owner(&fx.key()).unwrap();
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback), Err(HermesError::JournalFailed));
    // Restore refuses to overwrite the directory the backup slot now is.
    assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::RestoreFromBackup), Err(HermesError::JournalFailed));
    assert!(path.is_dir());
    assert!(matches!(fx.store.journal(&fx.key()).unwrap().unwrap().phase, Phase::CleanupPending));
    assert_eq!(inspect(&fx.cli, &fx.store, &fx.target), Ok(BindingHealth::RecoveryRequired));
    fs::remove_dir(&path).unwrap(); fs::rename(retained, path).unwrap();
    recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
    assert_eq!(*fx.cli.fields.borrow(), committed);
    assert_eq!(*fx.cli.writes.borrow(), writes);
    assert!(fx.store.owner(&fx.key()).unwrap() == owner);
    assert!(!fx.backup_exists());
    assert!(!fx.store.has_pending_operation().unwrap());
}

#[test]
fn lifecycle_artifacts_cleanup_rejects_foreign_target_or_config_path() {
    for foreign_target in [false, true] {
        let fx = Fixture::new();
        fx.store.interrupt_on(StoreWrite::Cleanup, 1);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err());
        let mut journal = fx.store.journal(&fx.key()).unwrap().unwrap();
        if foreign_target { journal.plan.selection.target.executable = fx.root.join("other-binary"); }
        else { journal.plan.snapshot.config_path = fx.root.join("outside-home.yaml"); }
        fx.store.save_journal(&fx.key(), &journal).unwrap();
        let before = fs::read(fx.root.join("store/registry.json")).unwrap();
        let committed = fx.cli.fields.borrow().clone();
        let writes = *fx.cli.writes.borrow();
        assert_eq!(recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback), Err(HermesError::RecoveryRequired));
        assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), before);
        assert!(fx.backup_exists());
        assert_eq!(*fx.cli.fields.borrow(), committed);
        assert_eq!(*fx.cli.writes.borrow(), writes);
    }
}

#[test]
fn lifecycle_artifacts_interrupted_private_temporary_is_recoverable() {
    let fx = Fixture::new();
    let before = fx.cli.fields.borrow().clone();
    fx.store.interrupt_on(StoreWrite::BackupTemporary, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err(), "no interruption inside the raw backup writer");
    let reopened = store::HermesStore::new(fx.root.join("store"));
    assert!(matches!(reopened.journal(&fx.key()).unwrap().unwrap().phase, Phase::PreparingBackup));
    let artifacts = fs::read_dir(fx.root.join("store/backups")).unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
    assert_eq!(artifacts.len(), 1);
    assert!(artifacts[0].file_name().unwrap().to_str().unwrap().starts_with('.'));
    assert_eq!(fs::read(&artifacts[0]).unwrap(), fx.cli.bytes());
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&artifacts[0]).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(inspect(&fx.cli, &reopened, &fx.target), Ok(BindingHealth::RecoveryRequired));
    recover(&fx.cli, &reopened, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert_eq!(*fx.cli.writes.borrow(), 0);
    assert!(!reopened.has_pending_operation().unwrap());
    assert!(!fx.backup_exists());
}

#[test]
fn lifecycle_artifacts_cleanup_removes_only_selected_targets_atomic_temporaries() {
    let fx = Fixture::new();
    fx.store.interrupt_on(StoreWrite::BackupSaved, 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.connect("team/first"))).is_err());
    let digest = format!("{:x}", sha2::Sha256::digest(fx.key().as_bytes()));
    let backups = fx.root.join("store/backups");
    // This is also the pre-upgrade atomic-writer naming convention.
    let temporary = backups.join(format!(".{digest}.raw.keyzapper-{}", uuid::Uuid::new_v4()));
    fs::write(&temporary, fx.cli.bytes()).unwrap();
    let unrelated = backups.join(format!(".{digest}.raw.keyzapper-not-a-uuid"));
    fs::write(&unrelated, b"unrelated").unwrap();
    let mut other = fx.target.clone();
    other.home = fx.root.join("other-home"); fs::create_dir(&other.home).unwrap();
    let other_key = store::HermesStore::target_key(&other).unwrap();
    let other_digest = format!("{:x}", sha2::Sha256::digest(other_key.as_bytes()));
    let mut journal = fx.store.journal(&fx.key()).unwrap().unwrap();
    journal.plan.selection.target = other.clone();
    journal.plan.snapshot.config_path = other.home.join("config.yaml");
    fx.store.save_journal(&other_key, &journal).unwrap();
    fx.store.save_raw_backup(&other, &fx.cli.bytes()).unwrap();
    let other_backup = backups.join(format!("{other_digest}.raw"));
    let other_temporary = backups.join(format!(".{other_digest}.raw.keyzapper-{}", uuid::Uuid::new_v4()));
    fs::write(&other_temporary, b"other active operation").unwrap();
    let before = fx.cli.fields.borrow().clone();
    recover(&fx.cli, &fx.store, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
    assert!(!temporary.exists(), "selected raw-config temporary was orphaned");
    assert!(!backups.join(format!("{digest}.raw")).exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
    assert_eq!(fs::read(other_temporary).unwrap(), b"other active operation");
    assert_eq!(fs::read(other_backup).unwrap(), fx.cli.bytes());
    assert!(fx.store.journal(&other_key).unwrap() == Some(journal));
    assert!(fx.store.journal(&fx.key()).unwrap().is_none());
    assert_eq!(*fx.cli.fields.borrow(), before);
    assert_eq!(*fx.cli.writes.borrow(), 0);
}

#[test]
fn lifecycle_pending_first_connect_without_owner_blocks_routing_identity_changes() {
    let fx = Fixture::new();
    let old = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into());
    let mut moved = old.clone();
    moved.endpoint = "https://gateway-b.test".into();
    let mut renamed = old.clone();
    renamed.name = "renamed".into();
    fx.store.fail_on(StoreWrite::Commit, 1);
    *fx.cli.fail_write.borrow_mut() = Some(5);
    assert_eq!(fx.connect("team/first"), Err(HermesError::RollbackFailed));
    assert!(fx.store.owner(&fx.key()).unwrap().is_none(), "a failed first connect must not commit an owner");
    assert!(fx.store.journal(&fx.key()).unwrap().is_some(), "a failed rollback must retain recovery");
    assert!(fx.cli.fields.borrow().contains_key("model.base_url"), "the gateway route was partially written");
    let config = fx.cli.bytes();
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    let reopened = store::HermesStore::new(fx.root.join("store"));
    let controller = controller::Controller::new(store::HermesStore::new(fx.root.join("store")));

    assert!(controller.expect_unbound_for_routing_change(&old, &moved).is_err(), "journal without owner still owns the written route");
    assert!(controller.has_pending_operation_for_profile(PROFILE).unwrap());
    assert!(!controller.has_pending_operation_for_profile("unrelated-profile").unwrap());
    assert!(controller.expect_unbound_for_routing_change(&old, &renamed).is_ok(), "a rename stays possible");
    assert_eq!(fx.cli.bytes(), config);
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);

    // The lifecycle fake stores its config as flat JSON; a production raw backup is YAML.
    // Replace it with the equivalent YAML so the real recovery validation runs on the
    // same on-disk shape the app would see.
    let yaml = b"model:\n  default: original-model\nproviders:\n  foreign:\n    unchanged: true\n";
    let mut journal = reopened.journal(&fx.key()).unwrap().unwrap();
    journal.plan.snapshot.config_digest = format!("{:x}", sha2::Sha256::digest(yaml));
    reopened.save_raw_backup(&fx.target, yaml).unwrap();
    reopened.save_journal(&fx.key(), &journal).unwrap();
    recover(&fx.cli, &reopened, &fx.target, &fx.cli.digest(), RecoveryAction::Rollback).unwrap();
    assert!(!reopened.has_pending_operation().unwrap());
    assert!(reopened.owner(&fx.key()).unwrap().is_none());
    assert!(!controller.has_pending_operation_for_profile(PROFILE).unwrap());
    assert!(controller.expect_unbound_for_routing_change(&old, &moved).is_ok(), "recovery releases the route");
}

#[test]
fn lifecycle_controller_backup_restore_is_a_distinct_confirmed_recovery_action() {
    struct Retarget<'a> { fake: &'a Fake, target: HermesTarget, restores: std::cell::Cell<usize> }
    impl ConfigAccess for Retarget<'_> {
        fn capability(&self, target: &HermesTarget) -> cli::Capability { assert_eq!(target, &self.target); cli::Capability::Supported }
        fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
        assert_eq!(target, &self.target); self.fake.snapshot(target, keys) }
        fn raw_config(&self, target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { self.fake.raw_config(target) }
        fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.fake.set(target, key, value) }
        fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()> { self.fake.unset(target, key) }
        fn restore_raw_config(&self, target: &HermesTarget, raw: &[u8], expected: &str) -> HermesResult<()> {
            self.restores.set(self.restores.get() + 1);
            assert_eq!(target, &self.target);
            if format!("{:x}", sha2::Sha256::digest(self.fake.bytes())) != expected { return Err(HermesError::Conflict); }
            // Replace the file exactly as the CLI transport does: raw bytes, mode, and digest.
            *self.fake.last_restore.borrow_mut() = Some(raw.to_vec());
            self.fake.restored.set(true);

            Ok(())
        }
    }
    let fx = Fixture::new();
    // Simulate a process death after the first forward write but before progress was saved:
    // the journal is `Applying`, the file already differs from the confirmed snapshot.
    let selection = HermesSelection { target: fx.target.clone(), profile_id: PROFILE.into(), model_id: "team/first".into(), tenant: None };
    let profile = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into());
    let plan = plan_connect(&fx.cli, selection, &profile, fx.root.join("keyzapper-helper").to_str().unwrap(), None, &ManagedConfig::default()).unwrap();
    let first = plan.changes.first().unwrap().clone();
    fx.cli.write(&first.key, match &first.after { StoredValue::Present(value) => Some(value), StoredValue::Absent => None }).unwrap();
    // The user-confirmed document, spelled exactly like the fake's raw config (production spells
    // this YAML) so recovery can inspect the result it expects.
    let backup = fixed(&serde_json::from_slice(&fx.cli.bytes()).unwrap());
    fx.store.save_raw_backup(&fx.target, &backup).unwrap();
    let mut plan = plan;
    // The backup is flat JSON even though production raw config is YAML; the plan has to name
    // exactly these bytes and their fields, or the restore would pass without writing anything.
    plan.snapshot.config_digest = format!("{:x}", sha2::Sha256::digest(&backup));
    let document: BTreeMap<String, Value> = serde_json::from_slice(&backup).unwrap();
    for key in recovery::OWNED_KEYS {
        let (section, field) = key.split_once('.').unwrap();
        let stored = document.get(section).and_then(|value| value.get(field)).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent);
        plan.snapshot.fields.insert(key.to_string(), stored.clone());
        if let Some(change) = plan.changes.iter_mut().find(|change| change.key == key) { change.before = stored; }
    }
    let pin = store::HermesStore::new(fx.root.join("store"));
    pin.save_journal(&fx.key(), &store::Journal { plan, completed: Vec::new(), phase: Phase::Applying, operation: store::Operation::Connect }).unwrap();
    let controller = controller::Controller::new(pin);
    let access = Retarget { fake: &fx.cli, target: fx.target.clone(), restores: std::cell::Cell::new(0) };
    let inputs = controller::Inputs {
        profiles: vec![Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into())],
        policy: ManagedConfig::default(), disabled: false, helper: Some(fx.root.join("keyzapper-helper").to_string_lossy().into_owned()),
        targets: vec![fx.target.clone()], diagnostics: vec![],
    };
    let preview = |action| controller.begin().unwrap().preview(&access, &inputs, action, &fx.target, None, std::time::Instant::now(), |_| {}).unwrap();
    assert_eq!(preview(controller::Action::Recover).recovery_action, Some(RecoveryAction::Rollback));
    assert_eq!(preview(controller::Action::RecoverFromBackup).recovery_action, Some(RecoveryAction::RestoreFromBackup));
    // A confirmation issued for a plain rollback does not authorize the whole-file restore.
    let rollback = preview(controller::Action::Recover);
    let restore = preview(controller::Action::RecoverFromBackup);
    let writes = *fx.cli.writes.borrow();
    assert!(controller.begin().unwrap().apply(&access, &inputs, controller::Action::RecoverFromBackup, &fx.target, None, &rollback.confirmation_id, std::time::Instant::now(), |_| {}).is_err());
    assert_eq!(access.restores.get(), 0, "a mismatched confirmation never reaches the transport");
    controller.begin().unwrap().apply(&access, &inputs, controller::Action::RecoverFromBackup, &fx.target, None, &restore.confirmation_id, std::time::Instant::now(), |_| {}).unwrap();
    assert_eq!(access.restores.get(), 1, "the confirmed restore went through the transport exactly once");
    assert_eq!(*fx.cli.writes.borrow(), writes, "restore never emits per-key writes");
    assert!(fx.cli.restored.get(), "the restore replaced the whole file instead of being skipped as a no-op");
    assert!(!controller.store().has_pending_operation().unwrap());
}

#[test]
fn lifecycle_corrupt_pending_registry_blocks_routing_changes_fail_closed() {
    let fx = Fixture::new();
    let old = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into());
    let mut moved = old.clone();
    moved.endpoint = "https://gateway-b.test".into();
    fs::create_dir_all(fx.root.join("store")).unwrap();
    fs::write(fx.root.join("store/registry.json"), b"synthetic-invalid-registry").unwrap();
    let registry = fs::read(fx.root.join("store/registry.json")).unwrap();
    let controller = controller::Controller::new(store::HermesStore::new(fx.root.join("store")));
    assert!(controller.expect_unbound_for_routing_change(&old, &moved).is_err());
    assert_eq!(fs::read(fx.root.join("store/registry.json")).unwrap(), registry);
}

#[test]
fn lifecycle_routing_change_is_allowed_after_disconnect() {
    let fx = Fixture::new();
    fx.connect("team/first").unwrap();
    let old = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), "team/first".into());
    let mut moved = old.clone();
    moved.endpoint = "https://gateway-b.test".into();
    let reopened = store::HermesStore::new(fx.root.join("store"));
    let controller = controller::Controller::new(store::HermesStore::new(fx.root.join("store")));
    assert!(controller.expect_unbound_for_routing_change(&old, &moved).is_err());
    reopened.commit(&fx.key(), None).unwrap();
    assert!(controller.expect_unbound_for_routing_change(&old, &moved).is_ok());
}
