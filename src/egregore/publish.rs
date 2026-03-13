//! Egregore publish client — HTTP client for /v1/publish endpoint.

use reqwest::Client;
use serde::Serialize;

use crate::egregore::messages::{
    CapabilityProof, Notification, ServitorProfile, TaskClaim, TaskFailed, TaskOffer,
    TaskOfferWithdraw, TaskResult, TaskStarted, TaskStatusMessage,
};
use crate::error::{Result, ServitorError};

/// Client for publishing messages to egregore.
pub struct EgregoreClient {
    http: Client,
    api_url: String,
}

impl EgregoreClient {
    /// Create a new egregore client.
    pub fn new(api_url: &str) -> Self {
        Self {
            http: Client::new(),
            api_url: api_url.trim_end_matches('/').to_string(),
        }
    }

    /// Get the API URL.
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    /// Publish a message to egregore.
    async fn publish<T: Serialize>(&self, content: &T, tags: &[&str]) -> Result<PublishedMessage> {
        let url = format!("{}/v1/publish", self.api_url);

        let request = PublishRequest {
            content: serde_json::to_value(content)?,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        };

        let response = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ServitorError::Egregore {
                reason: format!("publish request failed: {}", e),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Egregore {
                reason: format!("publish failed with {}: {}", status, body),
            });
        }

        let envelope: ApiResponse<PublishedMessage> =
            response.json().await.map_err(|e| ServitorError::Egregore {
                reason: format!("failed to parse publish response: {}", e),
            })?;

        envelope.data.ok_or_else(|| ServitorError::Egregore {
            reason: "publish response missing data field".into(),
        })
    }

    /// Publish a servitor profile (heartbeat/capability advertisement).
    pub async fn publish_profile(&self, profile: &ServitorProfile) -> Result<String> {
        let response = self.publish(profile, &["servitor_profile"]).await?;
        tracing::debug!(hash = %response.hash, "published servitor profile");
        Ok(response.hash)
    }

    /// Publish a task claim.
    pub async fn publish_claim(&self, claim: &TaskClaim) -> Result<String> {
        let response = self.publish(claim, &["task_claim"]).await?;
        tracing::debug!(
            hash = %response.hash,
            task_hash = %claim.task_hash,
            "published task claim"
        );
        Ok(response.hash)
    }

    /// Publish a task offer.
    pub async fn publish_offer(&self, offer: &TaskOffer) -> Result<String> {
        let response = self.publish(offer, &["task_offer"]).await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %offer.task_id,
            servitor = %offer.servitor,
            "published task offer"
        );
        Ok(response.hash)
    }

    /// Publish a task start acknowledgment.
    pub async fn publish_started(&self, started: &TaskStarted) -> Result<String> {
        let response = self.publish(started, &["task_started"]).await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %started.task_id,
            eta_seconds = started.eta_seconds,
            "published task started"
        );
        Ok(response.hash)
    }

    /// Publish a task status update.
    pub async fn publish_status(&self, status: &TaskStatusMessage) -> Result<String> {
        let response = self.publish(status, &["task_status"]).await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %status.task_id,
            "published task status"
        );
        Ok(response.hash)
    }

    /// Publish a task failure.
    pub async fn publish_failed(&self, failed: &TaskFailed) -> Result<String> {
        let response = self.publish(failed, &["task_failed"]).await?;
        tracing::info!(
            hash = %response.hash,
            task_id = %failed.task_id,
            reason = ?failed.reason,
            "published task failed"
        );
        Ok(response.hash)
    }

    /// Publish an offer withdrawal.
    pub async fn publish_offer_withdraw(&self, withdraw: &TaskOfferWithdraw) -> Result<String> {
        let response = self.publish(withdraw, &["task_offer_withdraw"]).await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %withdraw.task_id,
            "published task offer withdraw"
        );
        Ok(response.hash)
    }

    /// Publish a capability proof response.
    pub async fn publish_capability_proof(&self, proof: &CapabilityProof) -> Result<String> {
        let response = self.publish(proof, &["capability_proof"]).await?;
        tracing::debug!(
            hash = %response.hash,
            challenge_id = %proof.challenge_id,
            capability = %proof.capability,
            verified = proof.verified,
            "published capability proof"
        );
        Ok(response.hash)
    }

    /// Publish a task result.
    pub async fn publish_result(&self, result: &TaskResult) -> Result<String> {
        let tags = match result.status {
            crate::egregore::messages::TaskStatus::Success => vec!["task_result", "success"],
            crate::egregore::messages::TaskStatus::Error => vec!["task_result", "error"],
            crate::egregore::messages::TaskStatus::Timeout => vec!["task_result", "timeout"],
        };

        let response = self.publish(result, &tags).await?;
        tracing::info!(
            hash = %response.hash,
            task_hash = %result.task_hash,
            status = ?result.status,
            "published task result"
        );
        Ok(response.hash)
    }

    /// Publish a notification.
    pub async fn publish_notification(&self, notification: &Notification) -> Result<String> {
        let response = self.publish(notification, &["notification"]).await?;
        tracing::debug!(
            hash = %response.hash,
            channel = %notification.channel,
            "published notification"
        );
        Ok(response.hash)
    }

    /// Check if egregore is reachable.
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.api_url);
        match self.http.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

/// Publish request body.
#[derive(Debug, serde::Serialize)]
struct PublishRequest {
    content: serde_json::Value,
    tags: Vec<String>,
}

/// Egregore API response envelope.
#[derive(Debug, serde::Deserialize)]
struct ApiResponse<T> {
    #[allow(dead_code)]
    success: bool,
    data: Option<T>,
    #[allow(dead_code)]
    error: Option<ApiError>,
}

#[derive(Debug, serde::Deserialize)]
struct ApiError {
    #[allow(dead_code)]
    code: String,
    #[allow(dead_code)]
    message: String,
}

/// Published message data (subset of full Message).
#[derive(Debug, serde::Deserialize)]
struct PublishedMessage {
    hash: String,
    #[allow(dead_code)]
    sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PublicId;

    #[test]
    fn client_creation() {
        let client = EgregoreClient::new("http://127.0.0.1:7654");
        assert_eq!(client.api_url, "http://127.0.0.1:7654");
    }

    #[test]
    fn client_trims_trailing_slash() {
        let client = EgregoreClient::new("http://127.0.0.1:7654/");
        assert_eq!(client.api_url, "http://127.0.0.1:7654");
    }

    #[test]
    fn profile_serialization() {
        let profile = ServitorProfile::new(PublicId("@test.ed25519".to_string()), 10000);
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("servitor_profile"));
    }
}
