use sha2::Digest;
use std::{collections::HashMap, time::{Duration, Instant}};

/// Returns the persisted target keys bound to one profile; callers must fail closed if registry loading fails.
pub fn bound_profile_targets(ownership: &std::collections::BTreeMap<String, super::binding::Ownership>, profile_id: &str) -> Vec<String> {
    ownership.iter().filter(|(_, owner)| owner.selection.profile_id == profile_id).map(|(target, _)| target.clone()).collect()
}

/// Fields that bind a profile to a gateway route and credential identity. A rename or a key
/// rotation at the unchanged endpoint stays possible; changing any of these while a Hermes
/// target is bound would pair freshly minted credentials with the already-written old route.
pub fn routing_identity_changed(old: &crate::models::Profile, new: &crate::models::Profile) -> bool {
    old.endpoint != new.endpoint
        || old.auth_type != new.auth_type
        || old.oidc_issuer != new.oidc_issuer
        || old.oidc_client_id != new.oidc_client_id
        || old.oidc_scope != new.oidc_scope
        || old.oidc_token_type != new.oidc_token_type
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

const CONFIRMATION_TTL: Duration = Duration::from_secs(60);

pub fn confirmation_fingerprint(action: &str, target: &str, config_digest: &str, context: &str) -> String {
    digest(&serde_json::to_vec(&(action, target, config_digest, context)).expect("confirmation scope is serializable"))
}

#[derive(Default)]
pub struct Confirmations {
    entries: HashMap<String, (String, String, Instant)>,
}

impl Confirmations {
    pub fn issue(&mut self, action: &str, digest: &str, now: Instant) -> String {
        self.entries.retain(|_, (_, _, expires)| *expires > now);
        let id = uuid::Uuid::new_v4().to_string();
        self.entries.insert(id.clone(), (action.to_owned(), digest.to_owned(), now + CONFIRMATION_TTL));
        id
    }

    pub fn consume(&mut self, id: &str, action: &str, digest: &str, now: Instant) -> bool {
        let Some((expected_action, expected_digest, expires)) = self.entries.get(id) else { return false };
        if *expires <= now {
            self.entries.remove(id);
            return false;
        }
        if expected_action != action || expected_digest != digest { return false; }
        self.entries.remove(id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn bound_profile_targets_returns_only_exact_profile_bindings() {
        let selection = |profile_id: &str| super::super::binding::HermesSelection {
            target: super::super::HermesTarget { executable: "/opt/hermes".into(), home: "/tmp/home".into(), display_name: "home".into() },
            profile_id: profile_id.into(), model_id: "model".into(), tenant: None,
        };
        let ownership = std::collections::BTreeMap::from([
            ("target-a".into(), super::super::binding::Ownership { selection: selection("profile-a"), original: Default::default(), last_written: Default::default() }),
            ("target-b".into(), super::super::binding::Ownership { selection: selection("profile-b"), original: Default::default(), last_written: Default::default() }),
        ]);
        assert_eq!(bound_profile_targets(&ownership, "profile-a"), vec!["target-a"]);
        assert!(bound_profile_targets(&ownership, "missing").is_empty());
    }

    #[test]
    fn confirmation_is_bound_to_action_digest_expires_and_is_one_use() {
        let mut confirmations = Confirmations::default();
        let now = Instant::now();
        let id = confirmations.issue("connect:target-a:profile-a:model-a", "digest-a", now);

        assert!(!confirmations.consume(&id, "disconnect:target-a", "digest-a", now));
        assert!(!confirmations.consume(&id, "connect:target-a:profile-a:model-a", "digest-b", now));
        assert!(confirmations.consume(&id, "connect:target-a:profile-a:model-a", "digest-a", now));
        assert!(!confirmations.consume(&id, "connect:target-a:profile-a:model-a", "digest-a", now));

        let scope = confirmation_fingerprint("disconnect", "target-a", "digest-a", "policy-a");
        let other_target = confirmation_fingerprint("disconnect", "target-b", "digest-a", "policy-a");
        let other_action = confirmation_fingerprint("recover", "target-a", "digest-a", "policy-a");
        let other_context = confirmation_fingerprint("disconnect", "target-a", "digest-a", "policy-b");
        assert_ne!(scope, other_target);
        assert_ne!(scope, other_action);
        assert_ne!(scope, other_context);

        let scoped = confirmations.issue("disconnect:target-a", &scope, now);
        assert!(!confirmations.consume(&scoped, "disconnect:target-a", &other_target, now));
        assert!(confirmations.consume(&scoped, "disconnect:target-a", &scope, now));
        assert!(!confirmations.consume(&scoped, "disconnect:target-a", &scope, now));

        let other_profile = confirmation_fingerprint("connect", "target-a", "digest-a", "profile-b|policy-a|helper-a");
        let other_config = confirmation_fingerprint("connect", "target-a", "digest-b", "profile-a|policy-a|helper-a");
        let profile_bound = confirmation_fingerprint("connect", "target-a", "digest-a", "profile-a|policy-a|helper-a");
        let scoped_connect = confirmations.issue("connect:target-a", &profile_bound, now);
        assert!(!confirmations.consume(&scoped_connect, "connect:target-a", &other_profile, now));
        assert!(!confirmations.consume(&scoped_connect, "connect:target-a", &other_config, now));
        assert!(confirmations.consume(&scoped_connect, "connect:target-a", &profile_bound, now));

        let expired = confirmations.issue("recover:target-a", "digest-a", now);
        assert!(!confirmations.consume(&expired, "recover:target-a", "digest-a", now + Duration::from_secs(61)));
    }
}
