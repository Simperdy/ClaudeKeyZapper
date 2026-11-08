//! App actions shared by the desktop bridge and GTK-free tests. No credential access.
use super::{binding::plan_connect, service::{self, Confirmations}, store::HermesStore, ConfigAccess, HermesSelection, HermesTarget, RecoveryAction};
use crate::{l, managed::ManagedConfig, models::Profile};
use serde::Serialize;
use std::{sync::{Arc, Mutex}, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Connect, Disconnect, Recover }
impl Action {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value { "connect" => Ok(Self::Connect), "disconnect" => Ok(Self::Disconnect), "recover" => Ok(Self::Recover), _ => Err("Unknown Hermes action.".into()) }
    }
    fn name(self) -> &'static str { match self { Self::Connect => "connect", Self::Disconnect => "disconnect", Self::Recover => "recover" } }
}
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HermesTestStatus { #[default] NotTested, Succeeded, Failed }
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesView {
    pub targets: Vec<HermesTargetView>,
    pub diagnostics: Vec<super::cli::InstallationDiagnostic>,
    pub selected: Option<HermesSelection>,
    pub recovery_target: Option<HermesTarget>,
    pub health: String,
    pub problem: Option<String>,
    pub busy: bool,
    pub test_status: HermesTestStatus,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesTargetView { pub target: HermesTarget, pub supported: bool }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview { pub summary: String, pub confirmation_id: String, pub expires_in_seconds: u64 }

/// Taken under the UI model lock AFTER acquiring an operation permit. No I/O here.
pub struct Inputs {
    pub profiles: Vec<Profile>,
    pub policy: ManagedConfig,
    pub disabled: bool,
    pub helper: Option<String>,
    pub targets: Vec<HermesTarget>,
    pub diagnostics: Vec<super::cli::InstallationDiagnostic>,
}
#[derive(Default)]
struct State { active: bool, view: HermesView, confirmations: Confirmations }
struct Shared { state: Mutex<State>, store: HermesStore }
#[derive(Clone)]
pub struct Controller { shared: Arc<Shared> }
pub struct Operation { controller: Controller }
const UNSUPPORTED: &str = "Hermes build is unsupported or its local diagnostic failed. Writes are blocked: use an exactly validated installation and run the isolated test_hermes_contract.py diagnostic before connecting.";
fn error(error: super::HermesError) -> String {
    if error == super::HermesError::Unsupported { UNSUPPORTED.into() } else { format!("{error:?}") }
}
/// A stored binding only counts as configured while it still describes the current profile's
/// route and credential identity — a bare profile-ID match can be left behind by an external edit.
fn binding_matches_profile(owner: &super::Ownership, profile: &Profile) -> bool {
    let base_url = matches!(owner.last_written.get("model.base_url"), Some(super::StoredValue::Present(serde_json::Value::String(url))) if url == &profile.endpoint);
    let provider = owner.last_written.get("providers.keyzapper").is_some_and(|value| match value {
        super::StoredValue::Present(value) => super::cli::validate_provider_config(value)
            && value["base_url"].as_str() == Some(profile.endpoint.as_str())
            && value["key_cmd"].as_str().is_some_and(|cmd| cmd.contains(&format!(" credential --profile {} --json", profile.id))),
        super::StoredValue::Absent => false,
    });
    base_url && provider
}
impl Controller {
    pub fn new(store: HermesStore) -> Self { Self { shared: Arc::new(Shared { state: Mutex::new(State::default()), store }) } }
    pub fn store(&self) -> &HermesStore { &self.shared.store }
    pub fn view(&self) -> HermesView { self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).view.clone() }
    /// A bound profile may not change routing/identity fields before an explicit disconnect:
    /// the helper reads the profile fresh and would otherwise mint credentials for the new
    /// gateway towards the provider URL that is already written into Hermes.
    pub fn expect_unbound_for_routing_change(&self, old: &Profile, new: &Profile) -> Result<(), String> {
        if !service::routing_identity_changed(old, new) { return Ok(()); }
        // A pending journal can be a partially applied first connect whose rollback also
        // failed: the gateway route is written but no owner was committed, so the owner
        // check below cannot see it. Any pending operation conservatively blocks identity
        // changes until it is recovered; renames and same-route key rotation stay possible.
        if self.store().has_pending_operation().map_err(|_| l!("Hermes ownership registry is unreadable; profile changes are blocked."))? {
            return Err(l!("Recover Hermes before changing profiles."));
        }
        let ownership = self.store().load().map_err(|_| l!("Hermes ownership registry is unreadable; profile changes are blocked."))?;
        let targets = service::bound_profile_targets(&ownership, &old.id);
        if targets.is_empty() { return Ok(()); }
        Err(l!("Hermes vor dem Ändern von Gateway-Endpunkt, Anmeldeart oder OIDC-Identität dieses Profils trennen. Betroffene Ziele: %@", targets.join(", ")))
    }
    /// A locally absent profile can still have a partially written route: its pending
    /// journal names the profile ID even though no ownership entry was committed. Used by
    /// backup restore paths so a fresh identity is never authorized over that route.
    pub fn has_pending_operation_for_profile(&self, profile_id: &str) -> Result<bool, String> {
        let persisted = self.store().persisted_targets().map_err(|_| l!("Hermes ownership registry is unreadable; profile changes are blocked."))?;
        Ok(persisted.iter().any(|(_, _, journal)| {
            journal.as_ref().is_some_and(|journal| journal.plan.selection.profile_id == profile_id)
        }))
    }
    /// Hold through the entire profile mutation, not just this registry read.
    /// `None` is global deactivation; pending recovery conservatively blocks both.
    pub fn profile_change(&self, profile_id: Option<&str>, require_unbound: bool) -> Result<Operation, String> {
        let permit = self.begin()?;
        if require_unbound {
            let ownership = self.store().load().map_err(|_| l!("Hermes ownership registry is unreadable; profile changes are blocked."))?;
            if self.store().has_pending_operation().map_err(error)? { return Err(l!("Recover Hermes before changing profiles.")); }
            let targets = match profile_id {
                Some(id) => service::bound_profile_targets(&ownership, id),
                None => ownership.keys().cloned().collect(),
            };
            if !targets.is_empty() { return Err(l!("Disconnect Hermes before deleting this profile or deactivating KeyZapper. Affected targets: %@", targets.join(", "))); }
        }
        Ok(permit)
    }
    /// Nonblocking, including across Controller clones/model snapshots. The caller
    /// acquires this under the same Model lock used for profile mutation.
    pub fn begin(&self) -> Result<Operation, String> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.active { return Err(l!("Hermes action already in progress.")); }
        state.active = true;
        state.view.busy = true;
        Ok(Operation { controller: self.clone() })
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        let mut state = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = false;
        state.view.busy = false;
    }
}
struct Prepared { digest: String, fingerprint: String, summary: String, selection: HermesSelection }
impl Operation {
    /// Read-only status, including recovery detection. Never calls recover.
    pub fn refresh(self, access: &dyn ConfigAccess, inputs: &Inputs, mut publish: impl FnMut(HermesView)) -> Result<(), String> {
        publish(self.controller.view());
        let previous = self.controller.view().selected.map(|s| s.target);
        let result = self.read_view(access, inputs, previous.as_ref());
        self.finish(&result, &mut publish);
        result
    }
    fn read_view(&self, access: &dyn ConfigAccess, inputs: &Inputs, preferred: Option<&HermesTarget>) -> Result<(), String> {
        // Publish renewed capabilities before any fallible registry/config inspection.
        // A failed snapshot must never leave last refresh's Supported flags behind.
        let targets: Vec<_> = inputs.targets.iter().cloned().map(|target| {
            let supported = access.capability(&target) == super::cli::Capability::Supported;
            HermesTargetView { target, supported }
        }).collect();
        let mut diagnostics = inputs.diagnostics.clone();
        for entry in targets.iter().filter(|entry| !entry.supported) {
            if !diagnostics.iter().any(|item| item.executable == entry.target.executable) {
                diagnostics.push(super::cli::InstallationDiagnostic::for_executable(&entry.target.executable));
            }
        }
        diagnostics.retain(|item| !targets.iter().any(|entry| entry.supported && entry.target.executable == item.executable));
        {
            let mut state = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.view.targets = targets.clone();
            state.view.diagnostics = diagnostics;
        }
        let store = self.controller.store();
        let mut selected = None;
        let mut recovery_target = None;
        let mut inspected = preferred.filter(|target| inputs.targets.contains(target)).cloned();
        let persisted = store.persisted_targets().map_err(|_| l!("Hermes ownership registry is unreadable."))?;
        let indexed: std::collections::BTreeMap<_, _> = persisted.iter().map(|(key, owner, journal)| (key.as_str(), (owner, journal))).collect();
        let mut candidates = Vec::new();
        if let Some(target) = preferred.filter(|target| inputs.targets.contains(target)) {
            candidates.push((HermesStore::target_key(target).map_err(error)?, target.clone()));
        }
        for (key, _, journal) in &persisted {
            if let Some(journal) = journal { candidates.push((key.clone(), journal.plan.selection.target.clone())); }
        }
        for target in &inputs.targets {
            candidates.push((HermesStore::target_key(target).map_err(error)?, target.clone()));
        }
        for (key, owner, _) in &persisted {
            if let Some(owner) = owner { candidates.push((key.clone(), owner.selection.target.clone())); }
        }
        for (key, target) in candidates {
            let Some((owner, journal)) = indexed.get(key.as_str()) else { continue; };
            selected = owner.as_ref().map(|owner| owner.selection.clone());
            inspected = Some(target.clone());
            if journal.is_some() { recovery_target = Some(target); }
            break;
        }
        {
            let mut state = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.view.selected = selected.clone();
            state.view.recovery_target = recovery_target.clone();
        }
        let unavailable_recovery = recovery_target.as_ref().is_some_and(|target| !targets.iter().any(|entry| &entry.target == target && entry.supported));
        let status_target = inspected.as_ref().filter(|target| targets.iter().any(|entry| &entry.target == *target && entry.supported))
            .or_else(|| targets.iter().find(|entry| entry.supported).map(|entry| &entry.target))
            .or_else(|| inputs.targets.first());
        let health = if unavailable_recovery {
            // Keep repair for the persisted target visible while still probing an
            // unrelated supported installation below for its own current status.
            if let Some(target) = status_target { let _ = super::inspect(access, store, target).map_err(error)?; }
            "recoveryRequired"
        } else if let Some(target) = status_target {
            if !targets.iter().any(|entry| &entry.target == target && entry.supported) {
                if recovery_target.as_ref() == Some(target) { "recoveryRequired" } else { "unavailable" }
            } else {
                let profile = selected.as_ref().and_then(|s| inputs.profiles.iter().find(|p| p.id == s.profile_id));
                let owner = HermesStore::target_key(target).ok()
                    .and_then(|key| indexed.get(key.as_str()).and_then(|(owner, _)| owner.as_ref().cloned()));
                let route_matches = profile.is_some_and(|profile| owner.as_ref().is_some_and(|owner| binding_matches_profile(owner, profile)));
                match super::inspect(access, store, target).map_err(error)? {
                    super::BindingHealth::NotConnected => "notConnected",
                    // Not merely the profile ID: an external edit may have re-pointed the route.
                    super::BindingHealth::Configured if inputs.disabled || !route_matches => "unavailable",
                    super::BindingHealth::Configured => "configured",
                    super::BindingHealth::Drifted => "drifted",
                    super::BindingHealth::RecoveryRequired => "recoveryRequired",
                    super::BindingHealth::Unavailable => "unavailable",
                }
            }
        } else { "unavailable" };

        let mut state = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.view.targets = targets;
        state.view.selected = selected;
        state.view.recovery_target = recovery_target;
        state.view.health = health.into();
        state.view.problem = None; // Compatibility is target-scoped, not registry corruption.
        Ok(())
    }
    fn prepare(&self, access: &dyn ConfigAccess, inputs: &Inputs, action: Action, target: &HermesTarget, selection: Option<HermesSelection>) -> Result<Prepared, String> {
        if !inputs.targets.contains(target) { return Err(l!("Selected Hermes target is unavailable; refresh installations.")); }
        if access.capability(target) != super::cli::Capability::Supported { return Err(UNSUPPORTED.into()); }
        if selection.as_ref().is_some_and(|s| &s.target != target) { return Err("Selection target does not match preview target.".into()); }
        let store = self.controller.store();
        let key = HermesStore::target_key(target).map_err(error)?;
        let owner = store.owner(&key).map_err(error)?;
        let journal = store.journal(&key).map_err(error)?;
        let selected = match action {
            Action::Connect => selection.clone().ok_or_else(|| "Connect requires a selection.".to_string())?,
            Action::Disconnect => owner.as_ref().ok_or_else(|| "Hermes target is not connected.".to_string())?.selection.clone(),
            Action::Recover => journal.as_ref().ok_or_else(|| "No interrupted Hermes operation needs recovery.".to_string())?.plan.selection.clone(),
        };
        if &selected.target != target || selection.as_ref().is_some_and(|s| s != &selected) { return Err("Selection no longer matches the Hermes binding.".into()); }
        let profile = inputs.profiles.iter().find(|p| p.id == selected.profile_id);
        let (digest, summary) = match action {
            Action::Connect => {
                if inputs.disabled { return Err("KeyZapper is deactivated.".into()); }
                if journal.is_some() { return Err("RecoveryRequired".into()); }
                let profile = profile.ok_or_else(|| "Selected profile is unavailable.".to_string())?;
                let helper = inputs.helper.as_deref().ok_or_else(|| "KeyZapper helper is unavailable.".to_string())?;
                let plan = plan_connect(access, selected.clone(), profile, helper, owner.as_ref(), &inputs.policy).map_err(error)?;
                (plan.snapshot.config_digest, l!("Connect Hermes %@ using profile %@ and model %@.", target.display_name, profile.name, selected.model_id))
            }
            Action::Disconnect => {
                let keys = owner.as_ref().unwrap().last_written.keys().cloned().collect::<Vec<_>>();
                (access.snapshot(target, &keys).map_err(error)?.config_digest, l!("Disconnect KeyZapper from Hermes %@ and restore its owned settings.", target.display_name))
            }
            Action::Recover => {
                let phase = journal.as_ref().unwrap().phase;
                let summary = if matches!(phase, super::store::Phase::PreparingBackup | super::store::Phase::CleanupPending) {
                    l!("Clean up artifacts for the interrupted Hermes operation on %@.", target.display_name)
                } else { l!("Roll back the interrupted Hermes operation on %@.", target.display_name) };
                (access.snapshot(target, &[]).map_err(error)?.config_digest, summary)
            }
        };
        // Private state participates only in this digest, never in a DTO or error.
        let context = serde_json::to_string(&(target, &selected, profile.map(|p| (&p.id, &p.endpoint, p.auth_type)), if action == Action::Connect { inputs.helper.as_deref() } else { None }, &inputs.policy.allowed_gateway_hosts, inputs.disabled, &owner, &journal)).map_err(|_| "Invalid Hermes confirmation context.".to_string())?;
        let fingerprint = service::confirmation_fingerprint(action.name(), &key, &digest, &context);
        Ok(Prepared { digest, fingerprint, summary, selection: selected })
    }
    fn finish<T>(&self, result: &Result<T, String>, publish: &mut impl FnMut(HermesView)) {
        {
            let mut state = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.view.busy = false;
            if let Err(problem) = result { state.view.health = "problem".into(); state.view.problem = Some(problem.clone()); }
        }
        publish(self.controller.view());
    }
    pub fn preview(self, access: &dyn ConfigAccess, inputs: &Inputs, action: Action, target: &HermesTarget, selection: Option<HermesSelection>, now: Instant, mut publish: impl FnMut(HermesView)) -> Result<Preview, String> {
        publish(self.controller.view());
        let result = self.prepare(access, inputs, action, target, selection).map(|prepared| {
            let confirmation_id = self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner()).confirmations.issue(action.name(), &prepared.fingerprint, now);
            Preview { summary: prepared.summary, confirmation_id, expires_in_seconds: 60 }
        });
        self.finish(&result, &mut publish);
        result
    }
    pub fn apply(self, access: &dyn ConfigAccess, inputs: &Inputs, action: Action, target: &HermesTarget, selection: Option<HermesSelection>, confirmation_id: &str, now: Instant, mut publish: impl FnMut(HermesView)) -> Result<(), String> {
        let started = Instant::now();
        publish(self.controller.view());
        let mut result = (|| {
            let prepared = self.prepare(access, inputs, action, target, selection)?;
            // Preflight may block on the CLI. A token still has to be live AFTER
            // those reads, not only when the command entered the worker.
            if !self.controller.shared.state.lock().unwrap_or_else(|e| e.into_inner()).confirmations.consume(confirmation_id, action.name(), &prepared.fingerprint, now + started.elapsed()) {
                return Err("Confirmation expired or no longer matches this selection.".into());
            }
            match action {
                Action::Connect => super::connect(access, self.controller.store(), prepared.selection.clone(), inputs.profiles.iter().find(|p| p.id == prepared.selection.profile_id).unwrap(), inputs.helper.as_deref().unwrap(), &inputs.policy, &prepared.digest),
                Action::Disconnect => super::disconnect(access, self.controller.store(), target, &prepared.digest),
                Action::Recover => super::recover(access, self.controller.store(), target, &prepared.digest, RecoveryAction::Rollback),
            }.map_err(error)
        })();
        let refreshed = self.read_view(access, inputs, Some(target));
        if result.is_ok() { result = refreshed; }
        self.finish(&result, &mut publish);
        result
    }
}
