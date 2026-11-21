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
    /// Replace the whole config with validated raw bytes, but only while the current file still
    /// has `expected_digest` (compare-and-swap). Default: refuse rather than guess.
    fn restore_raw_config(&self, _target: &HermesTarget, _raw: &[u8], _expected_digest: &str) -> HermesResult<()> { Err(HermesError::Unsupported) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingHealth { NotConnected, Configured, Drifted, RecoveryRequired, Unavailable }
/// `Rollback` inverts our own completed writes and needs every owned field to hold either the
/// original or the written value. `RestoreFromBackup` replaces the whole file with the validated
/// raw backup after any third-party edit, including owned keys, because the user confirmed that
/// exact content by digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction { Rollback, RestoreFromBackup }

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
    if let Err(error) = apply_changes(access, store, &key, &mut journal) {
        if journal.completed.is_empty() { rollback(access, store, &key, &mut journal, previous.clone())?; return Err(error); }
        recover_forward_failure(access, store, &key, &mut journal, previous.clone(), error)?;
    }
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
    if let Err(error) = apply_changes(access, store, &key, &mut journal) {
        if journal.completed.is_empty() { rollback(access, store, &key, &mut journal, Some(owner.clone()))?; return Err(error); }
        recover_forward_failure(access, store, &key, &mut journal, Some(owner.clone()), error)?;
    }
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
    if matches!(journal.phase, Phase::PreparingBackup | Phase::CleanupPending | Phase::Restored) {
        // `Restored`: the backup already replaced the file; a retry only removes artifacts and
        // finishes the journal. The caller's confirmation still has to name the file in place,
        // so no third-party edit since the recovery preview can be swept away unseen.
        recovery::validate_cleanup_target(&journal, target)?;
        let current = access.snapshot(target, &[])?;
        if current.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
        if !matches!(journal.phase, Phase::Restored) { verify_raw_digest(access.raw_config(target)?.as_deref(), confirmed_digest)?; }
        if recovery::config_path(&current.config_path, target)? != recovery::config_path(&journal.plan.snapshot.config_path, target)? { return Err(HermesError::RecoveryRequired); }
        return cleanup(store, &key, target);
    }
    let owner = store.owner(&key)?;
    recovery::validate_journal(&journal, owner.as_ref(), target)?;
    let backup = store.raw_backup(target)?;
    recovery::validate_backup(&journal, &backup)?;
    let keys = recovery::OWNED_KEYS.iter().map(|key| key.to_string()).collect::<Vec<_>>();
    let current = access.snapshot(target, &keys)?;
    if current.config_digest != confirmed_digest { return Err(HermesError::Conflict); }
    verify_raw_digest(access.raw_config(target)?.as_deref(), confirmed_digest)?;
    if recovery::config_path(&current.config_path, target)? != recovery::config_path(&journal.plan.snapshot.config_path, target)? { return Err(HermesError::RecoveryRequired); }
    for key_name in &keys {
        let before = journal.plan.snapshot.fields.get(key_name);
        let after = journal.plan.changes.iter().find(|change| &change.key == key_name).map(|change| &change.after).or(before);
        let value = current.fields.get(key_name);
        // A third-party edit to an owned key never authorizes Rollback (it must not be overwritten
        // by an inverse write), but RestoreFromBackup makes no per-field promise: it replaces the
        // file with the exact content the user confirmed by digest, validated above.
        if (value.is_none() || (value != before && value != after)) && action == RecoveryAction::Rollback {
            journal.phase = Phase::RecoveryRequired;
            store.save_journal(&key, &journal)?;
            return Err(HermesError::RecoveryRequired);
        }
    }
    match action {
        RecoveryAction::Rollback => rollback(access, store, &key, &mut journal, owner),
        RecoveryAction::RestoreFromBackup => restore_from_backup(access, store, &key, &mut journal, owner, &backup),
    }
}

fn verify_raw_digest(raw: Option<&[u8]>, digest: &str) -> HermesResult<()> {
    let bytes = raw.ok_or(HermesError::InvalidConfig)?;
    if format!("{:x}", sha2::Sha256::digest(bytes)) == digest { Ok(()) } else { Err(HermesError::Conflict) }
}

/// The forward error is what the user acted on; a failed rollback is the more severe state and
/// carries the recovery obligation, so it wins.
fn combine(forward: HermesError, rollback: HermesError) -> HermesError {
    match rollback {
        // Nothing was written (or every write was reverted): the confirmed state still holds.
        _ if rollback == forward => forward,
        HermesError::Conflict => HermesError::Conflict,
        // A failed inverse write, an unverifiable value or an unreadable config all leave the
        // config in a state that only an explicit, confirmed recovery may resolve.
        _ => HermesError::RollbackFailed,
    }
}

/// A failed `apply_changes` after at least one of our writes reports the forward error once the
/// inverse writes ran (the helper returns `Err`; `Ok` means the inverse writes failed). When an
/// OWNED key changed under us mid-apply, `apply_changes` already says `RollbackFailed`: no inverse
/// write may clobber it, and the journal stays for an explicit recovery (rollback or restore).
fn recover_forward_failure(access: &dyn ConfigAccess, store: &store::HermesStore, key: &str, journal: &mut Journal, old_owner: Option<Ownership>, error: HermesError) -> HermesResult<()> {
    if error == HermesError::RollbackFailed { return Err(HermesError::RollbackFailed); }
    match rollback(access, store, key, journal, old_owner) { Ok(()) => Err(error), Err(rollback) => Err(combine(error, rollback)) }
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
/// Writes are authorized only while the FULL raw config still has the digest of the confirmed
/// snapshot: the per-key check alone cannot see concurrent edits to unmanaged keys, and a write
/// would silently clobber them. Each `write_value` is preceded by its own raw re-check, because
/// the CLI's own read-modify-write can race with any editor between two of our steps.
fn apply_changes(access: &dyn ConfigAccess, store: &store::HermesStore, key: &str, journal: &mut Journal) -> HermesResult<()> {
    journal.phase = Phase::Applying; store.save_journal(key, journal)?;
    let target = &journal.plan.selection.target;
    // Expected full-file digest: the confirmed snapshot, and after every step exactly that plus
    // our own completed write. Anything else in any key (managed or not) is a concurrent edit.
    let mut expected = journal.plan.snapshot.config_digest.clone();
    for change in &journal.plan.changes {
        match check_raw_digest(access, target, &expected) {
            Ok(()) => {},
            // After our first write the file legitimately differs; refuse loudly instead of guessing.
            Err(HermesError::Conflict) => return Err(if journal.completed.is_empty() { HermesError::Conflict } else { HermesError::RollbackFailed }),
            Err(error) => return Err(error),
        }
        let before = access.snapshot(target, std::slice::from_ref(&change.key))?;
        if before.config_digest != expected || before.fields.get(&change.key) != Some(&change.before) { return Err(HermesError::Conflict); }
        write_value(access, target, &change.key, &change.after)?;
        let readback = access.snapshot(target, std::slice::from_ref(&change.key))?;
        if readback.fields.get(&change.key) != Some(&change.after) { return Err(HermesError::CliFailed); }
        expected = readback.config_digest.clone();
        journal.completed.push(change.key.clone());
        if store.save_journal(key, journal).is_err() { return Err(HermesError::JournalFailed); }
    }
    // Every edit made after the last authorized write — including to unmanaged keys — shows up
    // here even when all owned fields read back exactly as written.
    match check_raw_digest(access, target, &expected) {
        Ok(()) => {},
        Err(HermesError::Conflict) => return Err(HermesError::RollbackFailed),
        Err(error) => return Err(error),
    }
    let mut desired = journal.plan.snapshot.fields.clone();
    for change in &journal.plan.changes { desired.insert(change.key.clone(), change.after.clone()); }
    let keys = desired.keys().cloned().collect::<Vec<_>>();
    let final_snapshot = access.snapshot(target, &keys)?;
    if final_snapshot.fields != desired { return Err(HermesError::CliFailed); }
    Ok(())
}
/// Raw bytes are authoritative for "nothing else changed"; a transport that cannot expose the
/// full file cannot prove the absence of concurrent edits and therefore refuses to write.
fn check_raw_digest(access: &dyn ConfigAccess, target: &HermesTarget, digest: &str) -> HermesResult<()> {
    verify_raw_digest(access.raw_config(target)?.as_deref(), digest)
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

/// Restore the exact validated raw backup over the target config. The caller has already proven
/// (validate_backup) that the bytes hash to the confirmed snapshot digest, parsed as one object,
/// and agree with every owned field, so this is exactly the state the user confirmed plus any
/// later third-party edits. Refusals:
/// - a config file that is not a regular file (symlink, FIFO, directory) is never overwritten;
/// - the backup is written through the transport twice and read back before the journal's
///   backup/cleanup artifacts are removed;
/// - a transport that cannot replace the raw file never gets silent best-effort semantics.
fn restore_from_backup(access: &dyn ConfigAccess, store: &store::HermesStore, key: &str, journal: &mut Journal, old_owner: Option<Ownership>, raw: &[u8]) -> HermesResult<()> {
    let target = &journal.plan.selection.target;
    let path = recovery::config_path(&journal.plan.snapshot.config_path, target)?;
    // Refuse to replace anything but a regular file (or a symlink the transport writes through);
    // a directory or device at the config path is never overwritten.
    let refuse = |journal: &mut Journal| -> HermesResult<()> { journal.phase = Phase::RecoveryRequired; let _ = store.save_journal(key, journal); Err(HermesError::RecoveryRequired) };
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() && !meta.file_type().is_symlink() => return refuse(journal),
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(_) => return refuse(journal),
    }
    let before = access.snapshot(target, &[])?;
    if recovery::config_path(&before.config_path, target)? != path { return refuse(journal); }
    let written = access.restore_raw_config(target, raw, &before.config_digest);
    // A transport that reports success must have left exactly the validated bytes behind; a
    // transport whose write failed is judged by what the file now contains. Compare the DIGEST,
    // not the spelling: a transport may normalize an equivalent YAML/JSON document.
    let expected = format!("{:x}", sha2::Sha256::digest(raw));
    let verified = written.and_then(|_| check_raw_digest(access, target, &expected));
    if let Err(error) = verified {
        // A refusal that left the file untouched keeps the recovery confirmable; anything else
        // needs a human decision about what the file now contains.
        let unchanged = match access.raw_config(target) { Ok(Some(now)) => format!("{:x}", sha2::Sha256::digest(now)) == before.config_digest, _ => false };
        journal.phase = if unchanged { Phase::RecoveryRequired } else { Phase::Restored };
        let _ = store.save_journal(key, journal);
        return Err(if error == HermesError::Unsupported { HermesError::RecoveryRequired } else { error });
    }
    journal.completed.clear();
    journal.phase = Phase::Restored;
    store.save_journal(key, journal)?;
    // Ownership lands with the cleanup obligation in one transaction, like connect/disconnect.
    // If that fails the journal stays `Restored` and the retried recovery only cleans up.
    if store.commit(key, old_owner).is_err() {
        let _ = store.save_journal(key, journal);
        return Err(HermesError::JournalFailed);
    }
    cleanup(store, key, target)
}
