//! Validate persisted recovery data before it can authorize inverse writes.
use super::{binding, cli, store::{HermesStore, Journal, Operation, Phase}, HermesError, HermesResult, HermesTarget, Ownership, StoredValue};
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};
use sha2::Digest;

pub(super) const OWNED_KEYS: [&str; 4] = ["model.base_url", "model.default", "model.provider", "providers.keyzapper"];

fn valid_selection(selection: &binding::HermesSelection) -> bool {
    crate::models::parse_id(&selection.profile_id).as_deref() == Some(selection.profile_id.as_str())
        && cli::safe_model_identifier(&selection.model_id)
}

fn valid_fields(fields: &BTreeMap<String, StoredValue>) -> bool {
    fields.len() == OWNED_KEYS.len() && OWNED_KEYS.iter().all(|key| match fields.get(*key) {
        Some(StoredValue::Absent) => true,
        Some(StoredValue::Present(value)) => cli::safe_existing_value(key, value),
        None => false,
    })
}

fn valid_managed(fields: &BTreeMap<String, StoredValue>, selection: &binding::HermesSelection) -> bool {
    if !valid_selection(selection) || !valid_fields(fields) { return false; }
    let StoredValue::Present(provider) = &fields["providers.keyzapper"] else { return false; };
    let Some(command) = provider.get("key_cmd").and_then(serde_json::Value::as_str) else { return false; };
    fields["model.provider"] == StoredValue::Present("keyzapper".into())
        && fields["model.default"] == StoredValue::Present(selection.model_id.clone().into())
        && provider.get("base_url").is_some_and(|url| fields["model.base_url"] == StoredValue::Present(url.clone()))
        && command.ends_with(&format!(" credential --profile {} --json", selection.profile_id))
}

pub(super) fn validate_owner(owner: &Ownership, target: &HermesTarget) -> HermesResult<()> {
    if HermesStore::target_key(&owner.selection.target)? != HermesStore::target_key(target)?
        || !valid_fields(&owner.original) || !valid_managed(&owner.last_written, &owner.selection)
        || owner.original["providers.keyzapper"] != StoredValue::Absent {
        return Err(HermesError::Conflict);
    }
    Ok(())
}

pub(super) fn config_path(path: &Path, target: &HermesTarget) -> HermesResult<PathBuf> {
    let home = fs::canonicalize(&target.home).map_err(|_| HermesError::WrongTarget)?;
    let resolved = if path.exists() { fs::canonicalize(path).map_err(|_| HermesError::WrongTarget)? }
        else { fs::canonicalize(path.parent().ok_or(HermesError::WrongTarget)?).map_err(|_| HermesError::WrongTarget)?.join(path.file_name().ok_or(HermesError::WrongTarget)?) };
    if !resolved.starts_with(home) { return Err(HermesError::WrongTarget); }
    Ok(resolved)
}

pub(super) fn validate_journal(journal: &Journal, owner: Option<&Ownership>, target: &HermesTarget) -> HermesResult<()> {
    let invalid = || HermesError::RecoveryRequired;
    let plan = &journal.plan;
    if HermesStore::target_key(&plan.selection.target).map_err(|_| invalid())? != HermesStore::target_key(target).map_err(|_| invalid())?
        || !valid_selection(&plan.selection) || !valid_fields(&plan.snapshot.fields)
        || plan.snapshot.config_digest.len() != 64 || !plan.snapshot.config_digest.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || !matches!(journal.phase, Phase::Prepared | Phase::Applying | Phase::RecoveryRequired) {
        return Err(invalid());
    }
    config_path(&plan.snapshot.config_path, target).map_err(|_| invalid())?;
    if let Some(owner) = owner {
        validate_owner(owner, target).map_err(|_| invalid())?;
        if plan.snapshot.fields != owner.last_written { return Err(invalid()); }
    } else if journal.operation == Operation::Disconnect || plan.snapshot.fields["providers.keyzapper"] != StoredValue::Absent {
        return Err(invalid());
    }
    let mut desired = plan.snapshot.fields.clone();
    let mut seen = BTreeSet::new();
    for change in &plan.changes {
        if !OWNED_KEYS.contains(&change.key.as_str()) || !seen.insert(&change.key)
            || plan.snapshot.fields.get(&change.key) != Some(&change.before) || change.before == change.after {
            return Err(invalid());
        }
        desired.insert(change.key.clone(), change.after.clone());
    }
    if !valid_fields(&desired) || journal.completed.len() > plan.changes.len()
        || journal.completed.iter().zip(&plan.changes).any(|(key, change)| key != &change.key) {
        return Err(invalid());
    }
    match journal.operation {
        Operation::Connect if valid_managed(&desired, &plan.selection) => Ok(()),
        Operation::Disconnect if owner.is_some_and(|owner| owner.selection == plan.selection && desired == owner.original) => Ok(()),
        _ => Err(invalid()),
    }
}

/// Cleanup has no inverse destinations to authorize. Validate only confinement;
/// it must also work before a backup exists and after ownership has changed.
pub(super) fn validate_cleanup_target(journal: &Journal, target: &HermesTarget) -> HermesResult<()> {
    if HermesStore::target_key(&journal.plan.selection.target).map_err(|_| HermesError::RecoveryRequired)?
        != HermesStore::target_key(target).map_err(|_| HermesError::RecoveryRequired)? {
        return Err(HermesError::RecoveryRequired);
    }
    config_path(&journal.plan.snapshot.config_path, target).map_err(|_| HermesError::RecoveryRequired)?;
    Ok(())
}

pub(super) fn validate_backup(journal: &Journal, raw: &[u8]) -> HermesResult<()> {
    if format!("{:x}", sha2::Sha256::digest(raw)) != journal.plan.snapshot.config_digest { return Err(HermesError::RecoveryRequired); }
    let parsed: serde_json::Value = if raw.is_empty() { serde_json::json!({}) }
        else { serde_yaml_ng::from_slice(raw).map_err(|_| HermesError::RecoveryRequired)? };
    if !parsed.is_object() { return Err(HermesError::RecoveryRequired); }
    for key in OWNED_KEYS {
        let value = key.split('.').try_fold(&parsed, |at, part| at.get(part));
        let stored = value.cloned().map(StoredValue::Present).unwrap_or(StoredValue::Absent);
        if journal.plan.snapshot.fields.get(key) != Some(&stored) { return Err(HermesError::RecoveryRequired); }
    }
    Ok(())
}
