//! Store failures are scheduled only in unit-test builds, never through a public API.
use super::*;
use super::store::StoreWrite;
use std::{cell::RefCell, fs};
use serde_json::json;

const PROFILE: &str = "123E4567-E89B-12D3-A456-426614174000";
struct Fake {
    fields: RefCell<BTreeMap<String, Value>>,
    writes: RefCell<usize>,
    fail_write: RefCell<Option<usize>>,
}
impl Fake {
    fn bytes(&self) -> Vec<u8> { serde_json::to_vec(&*self.fields.borrow()).unwrap() }
    fn digest(&self) -> String { format!("{:x}", sha2::Sha256::digest(self.bytes())) }
    fn write(&self, key: &str, value: Option<&Value>) -> HermesResult<()> {
        *self.writes.borrow_mut() += 1;
        if *self.fail_write.borrow() == Some(*self.writes.borrow()) { return Err(HermesError::CliFailed); }
        if let Some(value) = value { self.fields.borrow_mut().insert(key.into(), value.clone()); }
        else { self.fields.borrow_mut().remove(key); }
        Ok(())
    }
}
impl ConfigAccess for Fake {
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot> {
        Ok(ConfigSnapshot { config_path: target.home.join("config.yaml"), config_digest: self.digest(), fields: keys.iter().map(|key| (key.clone(), self.fields.borrow().get(key).cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent))).collect() })
    }
    fn raw_config(&self, _: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { Ok(Some(self.bytes())) }
    fn set(&self, _: &HermesTarget, key: &str, value: &Value) -> HermesResult<()> { self.write(key, Some(value)) }
    fn unset(&self, _: &HermesTarget, key: &str) -> HermesResult<()> { self.write(key, None) }
}
struct Fixture { root: PathBuf, target: HermesTarget, store: store::HermesStore, cli: Fake }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hermes-faults-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join("hermes"), b"synthetic").unwrap();
        let target = HermesTarget { executable: root.join("hermes"), home: root.join("home"), display_name: "fake".into() };
        Self { store: store::HermesStore::new(root.join("store")), root, target, cli: Fake { fields: RefCell::new(BTreeMap::from([("providers.foreign".into(), json!({"unchanged":true})), ("model.default".into(), json!("original-model"))])), writes: RefCell::new(0), fail_write: RefCell::new(None) } }
    }
    fn connect(&self, model: &str) -> HermesResult<()> {
        let selection = HermesSelection { target: self.target.clone(), profile_id: PROFILE.into(), model_id: model.into(), tenant: None };
        let profile = Profile::new(PROFILE.into(), "synthetic".into(), "https://gateway.test".into(), model.into());
        connect(&self.cli, &self.store, selection, &profile, "/opt/keyzapper-helper", &ManagedConfig::default(), &self.cli.digest())
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
    let plan = plan_connect(&fx.cli, selection, &profile, "/opt/keyzapper-helper", None, &ManagedConfig::default()).unwrap();
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
