use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use std::{collections::BTreeMap, path::PathBuf};
pub mod cli;
pub mod binding;
pub mod store;
pub mod service;
pub mod controller;
mod recovery;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod controller_tests;
pub use binding::{BindingPlan, FieldChange, HermesSelection, Ownership};
use binding::{ownership_after, plan_connect};
use store::{Journal, Operation, Phase};
use crate::{managed::ManagedConfig, models::Profile};

pub type HermesResult<T> = Result<T, HermesError>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesTarget { pub executable: PathBuf, pub home: PathBuf, pub display_name: String }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HermesError { MissingInstallation, Unsupported, WrongTarget, InvalidConfig, Collision, Conflict, RecoveryRequired, PolicyDenied, MissingCredential, SessionExpired, Timeout, CliFailed, RollbackFailed, JournalFailed }
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum StoredValue { Absent, Present(Value) }
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot { pub config_path: PathBuf, pub config_digest: String, pub fields: BTreeMap<String, StoredValue> }
pub trait ConfigAccess {
    fn capability(&self, _target: &HermesTarget) -> cli::Capability { cli::Capability::Unsupported }
    fn snapshot(&self, target: &HermesTarget, keys: &[String]) -> HermesResult<ConfigSnapshot>;
    fn raw_config(&self, _target: &HermesTarget) -> HermesResult<Option<Vec<u8>>> { Err(HermesError::Unsupported) }
    fn set(&self, target: &HermesTarget, key: &str, value: &Value) -> HermesResult<()>;
    fn unset(&self, target: &HermesTarget, key: &str) -> HermesResult<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingHealth { NotConnected, Configured, Drifted, RecoveryRequired, Unavailable }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction { Rollback }

/// Recompute from explicit inputs under the shared-home lock; a caller-supplied plan is never trusted.
pub fn connect(access: &dyn ConfigAccess, store: &store::HermesStore, selection: HermesSelection, profile: &Profile, helper_path: &str, policy: &ManagedConfig, confirmed_digest: &str) -> HermesResult<()> {
    if selection.profile_id != profile.id { return Err(HermesError::PolicyDenied); }
    let key = store::HermesStore::target_key(&selection.target)?;
    let _target_lock = store.lock_target(&selection.target)?;
    store.check_resource(&selection.target)?;
    if store.journal(&key)?.is_some() { return Err(HermesError::RecoveryRequired); }
    let previous = store.owner(&key)?;
    if let Some(owner) = &previous { recovery::validate_owner(owner, &selection.target)?; }
    let plan = plan_connect(access, selection, profile, helper_path, previous.as_ref(), policy)?;
    let keys = plan.snapshot.fields.keys().cloned().collect::<Vec<_>>();
    let fresh = access.snapshot(&plan.selection.target, &keys)?;
    if fresh.config_digest != plan.snapshot.config_digest || fresh.fields != plan.snapshot.fields || fresh.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
    let raw = access.raw_config(&plan.selection.target)?;
    verify_raw_digest(raw.as_deref(), &fresh.config_digest)?;
    let mut journal = Journal { plan: plan.clone(), completed: Vec::new(), phase: Phase::PreparingBackup, operation: Operation::Connect };
    prepare_backup(store, &key, &mut journal, raw.as_deref().ok_or(HermesError::InvalidConfig)?)?;
    if let Err(error) = apply_changes(access, store, &key, &mut journal) { rollback(access, store, &key, &mut journal, previous.clone())?; return Err(error); }
    let owner = ownership_after(&plan, previous.as_ref());
    if store.commit(&key, Some(owner)).is_err() { rollback(access, store, &key, &mut journal, previous)?; return Err(HermesError::JournalFailed); }
    cleanup(store, &key, &plan.selection.target)?;
    Ok(())
}

pub fn inspect(access: &dyn ConfigAccess, store: &store::HermesStore, target: &HermesTarget) -> HermesResult<BindingHealth> {
    let key = store::HermesStore::target_key(target)?;
    match store.check_resource(target) {
        Err(HermesError::RecoveryRequired | HermesError::Conflict) => return Ok(BindingHealth::RecoveryRequired),
        result => result?,
    }
    if store.journal(&key)?.is_some() { return Ok(BindingHealth::RecoveryRequired); }
    let Some(owner) = store.owner(&key)? else { return Ok(BindingHealth::NotConnected); };
    let keys = owner.last_written.keys().cloned().collect::<Vec<_>>();
    let snapshot = match access.snapshot(target, &keys) { Ok(v) => v, Err(HermesError::MissingInstallation | HermesError::WrongTarget | HermesError::InvalidConfig) => return Ok(BindingHealth::Unavailable), Err(e) => return Err(e) };
    if keys.iter().any(|key| snapshot.fields.get(key) != owner.last_written.get(key)) { Ok(BindingHealth::Drifted) } else { Ok(BindingHealth::Configured) }
}

pub fn disconnect(access: &dyn ConfigAccess, store: &store::HermesStore, target: &HermesTarget, confirmed_digest: &str) -> HermesResult<()> {
    let key = store::HermesStore::target_key(target)?;
    let _target_lock = store.lock_target(target)?;
    store.check_resource(target)?;
    if store.journal(&key)?.is_some() { return Err(HermesError::RecoveryRequired); }
    let owner = store.owner(&key)?.ok_or(HermesError::Conflict)?;
    recovery::validate_owner(&owner, target)?;
    let keys = owner.last_written.keys().cloned().collect::<Vec<_>>();
    let snapshot = access.snapshot(target, &keys)?;
    if snapshot.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
    for key in &keys { if snapshot.fields.get(key) != owner.last_written.get(key) { return Err(HermesError::Conflict); } }
    let changes = keys.iter().filter_map(|key| {
        let before = snapshot.fields.get(key)?.clone(); let after = owner.original.get(key)?.clone();
        (before != after).then(|| FieldChange { key: key.clone(), before, after })
    }).collect::<Vec<_>>();
    let plan = BindingPlan { selection: owner.selection.clone(), snapshot, changes };
    let raw = access.raw_config(target)?;
    verify_raw_digest(raw.as_deref(), &plan.snapshot.config_digest)?;
    let mut journal = Journal { plan, completed: Vec::new(), phase: Phase::PreparingBackup, operation: Operation::Disconnect };
    prepare_backup(store, &key, &mut journal, raw.as_deref().ok_or(HermesError::InvalidConfig)?)?;
    if let Err(error) = apply_changes(access, store, &key, &mut journal) { rollback(access, store, &key, &mut journal, Some(owner.clone()))?; return Err(error); }
    if store.commit(&key, None).is_err() {
        rollback(access, store, &key, &mut journal, Some(owner))?;
        return Err(HermesError::JournalFailed);
    }
    cleanup(store, &key, target)?;
    Ok(())
}

/// Confirmed recovery rolls back active writes, but preparation/committed cleanup
/// phases only remove artifacts and never acquire inverse-write authority.
pub fn recover(access: &dyn ConfigAccess, store: &store::HermesStore, target: &HermesTarget, confirmed_digest: &str, action: RecoveryAction) -> HermesResult<()> {
    let key = store::HermesStore::target_key(target)?;
    let _target_lock = store.lock_target(target)?;
    store.check_resource(target).map_err(|error| if error == HermesError::Conflict { HermesError::RecoveryRequired } else { error })?;
    let Some(mut journal) = store.journal(&key)? else { return Ok(()); };
    if matches!(journal.phase, Phase::PreparingBackup | Phase::CleanupPending) {
        recovery::validate_cleanup_target(&journal, target)?;
        let current = access.snapshot(target, &[])?;
        if current.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
        verify_raw_digest(access.raw_config(target)?.as_deref(), confirmed_digest)?;
        if recovery::config_path(&current.config_path, target)? != recovery::config_path(&journal.plan.snapshot.config_path, target)? { return Err(HermesError::RecoveryRequired); }
        return cleanup(store, &key, target);
    }
    let owner = store.owner(&key)?;
    recovery::validate_journal(&journal, owner.as_ref(), target)?;
    recovery::validate_backup(&journal, &store.raw_backup(target)?)?;
    let keys = recovery::OWNED_KEYS.iter().map(|key| key.to_string()).collect::<Vec<_>>();
    let current = access.snapshot(target, &keys)?;
    if current.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
    verify_raw_digest(access.raw_config(target)?.as_deref(), confirmed_digest)?;
    if recovery::config_path(&current.config_path, target)? != recovery::config_path(&journal.plan.snapshot.config_path, target)? { return Err(HermesError::RecoveryRequired); }
    for key_name in &keys {
        let before = journal.plan.snapshot.fields.get(key_name);
        let after = journal.plan.changes.iter().find(|change| &change.key == key_name).map(|change| &change.after).or(before);
        let value = current.fields.get(key_name);
        if value.is_none() || (value != before && value != after) {
            journal.phase = Phase::RecoveryRequired;
            store.save_journal(&key, &journal)?;
            return Err(HermesError::RecoveryRequired);
        }
    }
    match action {
        RecoveryAction::Rollback => {
            rollback(access, store, &key, &mut journal, owner)
        }
    }
}

fn verify_raw_digest(raw: Option<&[u8]>, digest: &str) -> HermesResult<()> {
    let bytes = raw.ok_or(HermesError::InvalidConfig)?;
    if format!("{:x}", sha2::Sha256::digest(bytes)) == digest { Ok(()) } else { Err(HermesError::Conflict) }
}

fn prepare_backup(store: &store::HermesStore, key: &str, journal: &mut Journal, raw: &[u8]) -> HermesResult<()> {
    // No raw file (including an atomic temporary) exists before this durable intent.
    store.save_journal(key, journal)?;
    let result = store.save_raw_backup(&journal.plan.selection.target, raw).and_then(|_| {
        journal.phase = Phase::Prepared;
        store.save_journal(key, journal)
    });
    if let Err(error) = result {
        cleanup(store, key, &journal.plan.selection.target)?;
        return Err(error);
    }
    Ok(())
}

fn cleanup(store: &store::HermesStore, key: &str, target: &HermesTarget) -> HermesResult<()> {
    store.remove_raw_backup(target)?;
    store.finish_cleanup(key)
}

fn write_value(access: &dyn ConfigAccess, target: &HermesTarget, key: &str, value: &StoredValue) -> HermesResult<()> {
    match value { StoredValue::Absent => access.unset(target, key), StoredValue::Present(value) => access.set(target, key, value) }
}
fn apply_changes(access: &dyn ConfigAccess, store: &store::HermesStore, key: &str, journal: &mut Journal) -> HermesResult<()> {
    journal.phase = Phase::Applying; store.save_journal(key, journal)?;
    for change in &journal.plan.changes {
        let before = access.snapshot(&journal.plan.selection.target, std::slice::from_ref(&change.key))?;
        if before.fields.get(&change.key) != Some(&change.before) { return Err(HermesError::Conflict); }
        write_value(access, &journal.plan.selection.target, &change.key, &change.after)?;
        let readback = access.snapshot(&journal.plan.selection.target, std::slice::from_ref(&change.key))?;
        if readback.fields.get(&change.key) != Some(&change.after) { return Err(HermesError::CliFailed); }
        journal.completed.push(change.key.clone());
        if store.save_journal(key, journal).is_err() { return Err(HermesError::JournalFailed); }
    }
    let mut desired = journal.plan.snapshot.fields.clone();
    for change in &journal.plan.changes { desired.insert(change.key.clone(), change.after.clone()); }
    let keys = desired.keys().cloned().collect::<Vec<_>>();
    let final_snapshot = access.snapshot(&journal.plan.selection.target, &keys)?;
    if final_snapshot.fields != desired { return Err(HermesError::CliFailed); }
    Ok(())
}
fn rollback(access: &dyn ConfigAccess, store: &store::HermesStore, key: &str, journal: &mut Journal, old_owner: Option<Ownership>) -> HermesResult<()> {
    let target = &journal.plan.selection.target;
    for change in journal.plan.changes.iter().rev() {
        let now = access.snapshot(target, std::slice::from_ref(&change.key))?;
        let current = now.fields.get(&change.key).ok_or(HermesError::RollbackFailed)?;
        if current == &change.before { continue; }
        if current != &change.after { journal.phase = Phase::RecoveryRequired; let _ = store.save_journal(key, journal); return Err(HermesError::RollbackFailed); }
        if write_value(access, target, &change.key, &change.before).is_err() { journal.phase = Phase::RecoveryRequired; let _ = store.save_journal(key, journal); return Err(HermesError::RollbackFailed); }
        let check = access.snapshot(target, std::slice::from_ref(&change.key))?;
        if check.fields.get(&change.key) != Some(&change.before) { journal.phase = Phase::RecoveryRequired; let _ = store.save_journal(key, journal); return Err(HermesError::RollbackFailed); }
    }
    store.commit(key, old_owner).map_err(|_| HermesError::RollbackFailed)?;
    cleanup(store, key, target)?;
    Ok(())
}
