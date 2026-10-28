//! Authorization event publishing utilities.

use crate::authority::{Authority, DenialReason, PersonId};
use crate::egregore::{AuthDenied, AuthGate, EgregoreClient};
use crate::identity::Identity;
use crate::metrics::{self, AuthDecision};

/// Publish an authorization denial event to the egregore network.
pub async fn publish_auth_denied_event(
    egregore: &EgregoreClient,
    identity: &Identity,
    authority: &Authority,
    person: &PersonId,
    skill: &str,
    gate: AuthGate,
    reason: DenialReason,
) {
    // Record auth denial metric
    metrics::record_auth_decision(AuthDecision::Denied);

    let denial = auth_denied_record(identity, authority, person, skill, gate, reason);

    if let Err(error) = egregore.publish_auth_denied(&denial).await {
        tracing::debug!(error = %error, skill = %skill, "failed to publish auth denial");
    }
}

/// Build the `auth_denied` record for a refused request. The record carries
/// a reason code and an approved skill label, never requestor text.
fn auth_denied_record(
    identity: &Identity,
    authority: &Authority,
    person: &PersonId,
    skill: &str,
    gate: AuthGate,
    reason: DenialReason,
) -> AuthDenied {
    let person_id = match person {
        PersonId::Egregore(pubkey) => pubkey.clone(),
        PersonId::Discord(user_id) => format!("discord:{user_id}"),
        PersonId::Http(_) => "http:<redacted>".to_string(),
    };
    AuthDenied::new(
        identity.public_id(),
        person_id,
        authority.public_skill_label(skill),
        gate,
        reason,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{AuthRequest, AuthorityConfig};
    use crate::egregore::Task;

    fn authority() -> Authority {
        Authority::from_config(
            AuthorityConfig::from_toml(
                r#"
[[keeper]]
name = "operator"
egregore = "@operator.ed25519"

[[permission]]
keeper = "operator"
skills = ["request:deploy"]
"#,
            )
            .unwrap(),
        )
    }

    fn task_of_type(task_type: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "type": "task",
            "hash": "h",
            "prompt": "p",
            "task_type": task_type,
        }))
        .unwrap()
    }

    /// A refused request's task type is the requestor's own text, so the
    /// published record names neither it nor a reason built from it.
    #[test]
    fn denial_record_does_not_echo_the_requested_task_type() {
        let authority = authority();
        let task = task_of_type("leak-me-7f3");
        let skill = crate::task::request_skill(&task);
        for person in [
            PersonId::from_egregore("@operator.ed25519"),
            PersonId::from_egregore("@stranger.ed25519"),
        ] {
            let result = authority.authorize(&AuthRequest {
                person: person.clone(),
                skill: skill.clone(),
            });
            assert!(!result.allowed);
            let record = auth_denied_record(
                &Identity::generate(),
                &authority,
                &person,
                &skill,
                AuthGate::Offer,
                result.denial.unwrap(),
            );
            let json = serde_json::to_string(&record).unwrap();
            assert!(!json.contains("leak-me"), "{json}");
            assert!(json.contains("\"skill\":\"request:other\""), "{json}");
        }
    }
}
