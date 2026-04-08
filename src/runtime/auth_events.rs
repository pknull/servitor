//! Authorization event publishing utilities.

use crate::authority::PersonId;
use crate::egregore::{AuthDenied, AuthGate, EgregoreClient};
use crate::identity::Identity;
use crate::metrics::{self, AuthDecision};

/// Publish an authorization denial event to the egregore network.
pub async fn publish_auth_denied_event(
    egregore: &EgregoreClient,
    identity: &Identity,
    person: &PersonId,
    skill: &str,
    gate: AuthGate,
    reason: &str,
) {
    // Record auth denial metric
    metrics::record_auth_decision(AuthDecision::Denied);

    let person_id = match person {
        PersonId::Egregore(pubkey) => pubkey.clone(),
        PersonId::Discord(user_id) => format!("discord:{user_id}"),
        PersonId::Http(_) => "http:<redacted>".to_string(),
    };

    let denial = AuthDenied::new(
        identity.public_id(),
        person_id,
        skill.to_string(),
        gate,
        reason.to_string(),
    );

    if let Err(error) = egregore.publish_auth_denied(&denial).await {
        tracing::debug!(error = %error, skill = %skill, "failed to publish auth denial");
    }
}
