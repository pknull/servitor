//! Egregore publish client — HTTP client for /v1/publish endpoint.

use reqwest::Client;
use serde::Serialize;

use crate::egregore::messages::{Notification, ServitorProfile, TaskClaim, TaskResult};
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

    /// Publish a message to egregore.
    async fn publish<T: Serialize>(&self, content: &T, tags: &[&str]) -> Result<PublishResponse> {
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

        let result: PublishResponse = response.json().await.map_err(|e| ServitorError::Egregore {
            reason: format!("failed to parse publish response: {}", e),
        })?;

        Ok(result)
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

/// Publish response.
#[derive(Debug, serde::Deserialize)]
struct PublishResponse {
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
        let profile = ServitorProfile::new(
            PublicId("@test.ed25519".to_string()),
            10000,
        );
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("servitor_profile"));
    }
}
