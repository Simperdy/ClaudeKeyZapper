use super::{cli, ConfigAccess, ConfigSnapshot, HermesError, HermesResult, StoredValue};
use crate::{managed::ManagedConfig, models::{AuthType, Profile, parse_id}};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSelection { pub target: super::HermesTarget, pub profile_id: String, pub model_id: String, pub tenant: Option<String> }
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange { pub key: String, pub before: StoredValue, pub after: StoredValue }
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingPlan { pub selection: HermesSelection, pub snapshot: ConfigSnapshot, pub changes: Vec<FieldChange> }

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Ownership { pub selection: HermesSelection, pub original: BTreeMap<String, StoredValue>, pub last_written: BTreeMap<String, StoredValue> }

pub fn desired_provider(profile: &Profile, helper_path: &str) -> HermesResult<Value> {
    if profile.auth_type == AuthType::Subscription { return Err(HermesError::PolicyDenied); }
    let helper = Path::new(helper_path);
    cli::desired_provider(&profile.endpoint, helper, &profile.id).ok_or(HermesError::InvalidConfig)
}

pub fn plan_connect(access: &dyn ConfigAccess, selection: HermesSelection, profile: &Profile, helper_path: &str, owned: Option<&Ownership>, policy: &ManagedConfig) -> HermesResult<BindingPlan> {
    if selection.profile_id != profile.id || parse_id(&selection.profile_id).as_deref() != Some(selection.profile_id.as_str())
        || selection.model_id.trim().is_empty() || profile.auth_type == AuthType::Subscription
        || !policy.is_endpoint_allowed(&profile.endpoint) { return Err(HermesError::PolicyDenied); }
    // An explicitly selected tenant must be the exact `<tenant>/` prefix of the written model ID.
    // API callers pass through here too, so this is not a DOM-only check; nothing is rewritten.
    if let Some(tenant) = selection.tenant.as_deref() {
        let matches = selection.model_id.strip_prefix(tenant).and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|model| !model.is_empty());
        if tenant.trim().is_empty() || tenant.trim() != tenant || !matches { return Err(HermesError::PolicyDenied); }
    }
    let provider = desired_provider(profile, helper_path)?;
    if !cli::safe_model_identifier(&selection.model_id) { return Err(HermesError::InvalidConfig); }
    let desired = BTreeMap::from([
        ("providers.keyzapper".to_string(), StoredValue::Present(provider)),
        ("model.provider".to_string(), StoredValue::Present(Value::String("keyzapper".into()))),
        ("model.default".to_string(), StoredValue::Present(Value::String(selection.model_id.clone()))),
        ("model.base_url".to_string(), StoredValue::Present(Value::String(profile.endpoint.clone()))),
    ]);
    let keys = desired.keys().cloned().collect::<Vec<_>>();
    if let Some(owner) = owned {
        if owner.selection.target.home != selection.target.home || owner.selection.target.executable != selection.target.executable
            || keys.iter().any(|key| !owner.original.contains_key(key) || !owner.last_written.contains_key(key)) {
            return Err(HermesError::Conflict);
        }
    }
    let snapshot = access.snapshot(&selection.target, &keys)?;
    if keys.iter().any(|key| !snapshot.fields.contains_key(key)) { return Err(HermesError::InvalidConfig); }
    let mut changes = Vec::new();
    for (key, after) in &desired {
        let before = snapshot.fields[key].clone();
        if owned.is_none() && key == "providers.keyzapper" && before != StoredValue::Absent { return Err(HermesError::Collision); }
        safe_original(key, &before)?;
        if let Some(owner) = owned {
            if let Some(last) = owner.last_written.get(key) {
                if &before != last { return Err(HermesError::Conflict); }
                if let Some(original) = owner.original.get(key) { safe_original(key, original)?; }
            }
        }
        if before != *after { changes.push(FieldChange { key: key.clone(), before, after: after.clone() }); }
    }
    Ok(BindingPlan { selection, snapshot, changes })
}

pub fn ownership_after(plan: &BindingPlan, previous: Option<&Ownership>) -> Ownership {
    let mut original = previous.map(|owner| owner.original.clone()).unwrap_or_default();
    let mut last_written = previous.map(|owner| owner.last_written.clone()).unwrap_or_default();
    if previous.is_none() {
        for (key, before) in &plan.snapshot.fields {
            original.insert(key.clone(), before.clone());
            last_written.insert(key.clone(), before.clone());
        }
    }
    for change in &plan.changes {
        original.entry(change.key.clone()).or_insert_with(|| change.before.clone());
        last_written.insert(change.key.clone(), change.after.clone());
    }
    Ownership { selection: plan.selection.clone(), original, last_written }
}

fn safe_original(key: &str, value: &StoredValue) -> HermesResult<()> {
    if let StoredValue::Present(value) = value {
        if !cli::safe_existing_value(key, value) { return Err(HermesError::InvalidConfig); }
    }
    Ok(())
}
