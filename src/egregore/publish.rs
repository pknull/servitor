//! Egregore publish client — HTTP client for /v1/publish endpoint.

use reqwest::Client;
use serde::Serialize;

use crate::egregore::messages::{
    AuthDenied, EnvironmentSnapshot, Notification, ServitorManifest, ServitorProfile, TaskClaim,
    TaskFailed, TaskOffer, TaskOfferWithdraw, TaskResult, TaskStarted, TaskStatusMessage,
    TraceSpan,
};
use crate::error::{Result, ServitorError};

/// Client for publishing messages to egregore.
#[derive(Clone)]
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
    async fn publish<T: Serialize>(
        &self,
        content: &T,
        tags: &[&str],
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<PublishedMessage> {
        let url = format!("{}/v1/publish", self.api_url);

        let request = PublishRequest {
            content: serde_json::to_value(content)?,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            trace_id: trace_id.map(str::to_string),
            span_id: span_id.map(str::to_string),
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
        let response = self
            .publish(profile, &["servitor_profile"], None, None)
            .await?;
        tracing::debug!(hash = %response.hash, "published servitor profile");
        Ok(response.hash)
    }

    /// Publish a planner-facing servitor manifest.
    pub async fn publish_manifest(&self, manifest: &ServitorManifest) -> Result<String> {
        let response = self
            .publish(manifest, &["servitor_manifest"], None, None)
            .await?;
        tracing::debug!(hash = %response.hash, "published servitor manifest");
        Ok(response.hash)
    }

    /// Publish a planner-facing environment snapshot.
    pub async fn publish_environment_snapshot(
        &self,
        snapshot: &EnvironmentSnapshot,
    ) -> Result<String> {
        let response = self
            .publish(snapshot, &["environment_snapshot"], None, None)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            target_id = %snapshot.target_id,
            "published environment snapshot"
        );
        Ok(response.hash)
    }

    /// Publish a task claim.
    pub async fn publish_claim(&self, claim: &TaskClaim) -> Result<String> {
        let response = self.publish(claim, &["task_claim"], None, None).await?;
        tracing::debug!(
            hash = %response.hash,
            task_hash = %claim.task_hash,
            "published task claim"
        );
        Ok(response.hash)
    }

    /// Publish a task offer with optional trace context.
    pub async fn publish_offer(
        &self,
        offer: &TaskOffer,
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<String> {
        let response = self
            .publish(offer, &["task_offer"], trace_id, span_id)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %offer.task_id,
            servitor = %offer.servitor,
            "published task offer"
        );
        Ok(response.hash)
    }

    /// Publish a task start acknowledgment with optional trace context.
    pub async fn publish_started(
        &self,
        started: &TaskStarted,
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<String> {
        let response = self
            .publish(started, &["task_started"], trace_id, span_id)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %started.task_id,
            eta_seconds = started.eta_seconds,
            "published task started"
        );
        Ok(response.hash)
    }

    /// Publish a task status update with optional trace context.
    pub async fn publish_status(
        &self,
        status: &TaskStatusMessage,
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<String> {
        let response = self
            .publish(status, &["task_status"], trace_id, span_id)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %status.task_id,
            "published task status"
        );
        Ok(response.hash)
    }

    /// Publish a task failure with optional trace context.
    pub async fn publish_failed(
        &self,
        failed: &TaskFailed,
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<String> {
        let response = self
            .publish(failed, &["task_failed"], trace_id, span_id)
            .await?;
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
        let response = self
            .publish(withdraw, &["task_offer_withdraw"], None, None)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            task_id = %withdraw.task_id,
            "published task offer withdraw"
        );
        Ok(response.hash)
    }

    /// Publish an authorization denial event.
    pub async fn publish_auth_denied(&self, denial: &AuthDenied) -> Result<String> {
        let gate_tag = match denial.gate {
            crate::egregore::messages::AuthGate::Offer => "offer",
            crate::egregore::messages::AuthGate::Assignment => "assignment",
        };
        let response = self
            .publish(denial, &["auth_denied", gate_tag], None, None)
            .await?;
        tracing::info!(
            hash = %response.hash,
            person_id = %denial.person_id,
            skill = %denial.skill,
            "published auth denial"
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

        let response = self
            .publish(result, &tags, result.trace_id.as_deref(), None)
            .await?;
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
        let response = self
            .publish(notification, &["notification"], None, None)
            .await?;
        tracing::debug!(
            hash = %response.hash,
            channel = %notification.channel,
            "published notification"
        );
        Ok(response.hash)
    }

    /// Publish a trace span message.
    pub async fn publish_trace_span(&self, span: &TraceSpan) -> Result<String> {
        let status_tag = match span.status {
            crate::egregore::messages::TraceSpanStatus::Ok => "ok",
            crate::egregore::messages::TraceSpanStatus::Error => "error",
            crate::egregore::messages::TraceSpanStatus::Timeout => "timeout",
        };

        let response = self
            .publish(
                span,
                &["trace_span", status_tag],
                Some(&span.trace_id),
                Some(&span.span_id),
            )
            .await?;
        tracing::debug!(
            hash = %response.hash,
            trace_id = %span.trace_id,
            span_id = %span.span_id,
            name = %span.name,
            "published trace span"
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

    /// Query messages from egregore by content type.
    pub async fn query_messages(
        &self,
        content_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::egregore::EgregoreMessage>> {
        let mut url = format!("{}/v1/query?limit={}", self.api_url, limit);
        if let Some(ct) = content_type {
            url.push_str(&format!("&content_type={}", ct));
        }

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ServitorError::Egregore {
                reason: format!("query request failed: {}", e),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Egregore {
                reason: format!("query failed with {}: {}", status, body),
            });
        }

        let envelope: ApiResponse<Vec<crate::egregore::EgregoreMessage>> =
            response.json().await.map_err(|e| ServitorError::Egregore {
                reason: format!("failed to parse query response: {}", e),
            })?;

        Ok(envelope.data.unwrap_or_default())
    }
}

/// Publish request body.
#[derive(Debug, serde::Serialize)]
struct PublishRequest {
    content: serde_json::Value,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_id: Option<String>,
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
