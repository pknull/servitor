//! HTTP A2A client — JSON-RPC 2.0 over HTTPS.
//!
//! Implements A2A protocol for communicating with external agent services.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, header};
use tokio::time::sleep;

use super::card::AgentCard;
use super::client::*;
use super::{A2aError, Result};

/// A2A client using HTTP transport.
pub struct HttpA2aClient {
    name: String,
    url: String,
    card_url: String,
    http: Client,
    request_id: AtomicU64,
    cached_card: Option<AgentCard>,
    timeout_secs: u64,
    poll_interval_ms: u64,
}

/// Configuration for creating an HTTP A2A client.
pub struct HttpA2aConfig {
    /// Agent name (for tool prefixing).
    pub name: String,
    /// Base URL for agent API.
    pub url: String,
    /// URL for agent card (defaults to {url}/.well-known/agent.json).
    pub card_url: Option<String>,
    /// Bearer token for authentication.
    pub bearer_token: Option<String>,
    /// Timeout in seconds for task completion.
    pub timeout_secs: u64,
    /// Poll interval in milliseconds.
    pub poll_interval_ms: u64,
}

impl HttpA2aClient {
    /// Create a new HTTP A2A client.
    pub fn new(config: HttpA2aConfig) -> Result<Self> {
        let mut headers = header::HeaderMap::new();

        if let Some(token) = &config.bearer_token {
            let value = header::HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| A2aError::AuthFailed {
                    reason: format!("invalid bearer token: {}", e),
                })?;
            headers.insert(header::AUTHORIZATION, value);
        }

        let http = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(A2aError::Http)?;

        let card_url = config.card_url.unwrap_or_else(|| {
            format!("{}/.well-known/agent.json", config.url.trim_end_matches('/'))
        });

        Ok(Self {
            name: config.name,
            url: config.url,
            card_url,
            http,
            request_id: AtomicU64::new(1),
            cached_card: None,
            timeout_secs: config.timeout_secs,
            poll_interval_ms: config.poll_interval_ms,
        })
    }

    /// Send a JSON-RPC request.
    async fn rpc<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        // A2A JSON-RPC endpoint is at /a2a
        let rpc_url = format!("{}/a2a", self.url.trim_end_matches('/'));

        tracing::debug!(
            agent = %self.name,
            method = %method,
            url = %rpc_url,
            "sending A2A request"
        );

        let response = self
            .http
            .post(&rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(A2aError::Http)?;

        if !response.status().is_success() {
            return Err(A2aError::Protocol {
                reason: format!("HTTP error: {}", response.status()),
            });
        }

        let rpc_response: JsonRpcResponse = response.json().await.map_err(A2aError::Http)?;

        if rpc_response.id != id {
            return Err(A2aError::Protocol {
                reason: format!(
                    "response ID mismatch: expected {}, got {}",
                    id, rpc_response.id
                ),
            });
        }

        if let Some(error) = rpc_response.error {
            return Err(A2aError::Protocol {
                reason: format!("A2A error {}: {}", error.code, error.message),
            });
        }

        let result = rpc_response.result.ok_or_else(|| A2aError::Protocol {
            reason: "response missing result".into(),
        })?;

        serde_json::from_value(result).map_err(A2aError::Json)
    }

    /// Poll task until terminal state or timeout.
    async fn poll_until_complete(&self, task_id: &str) -> Result<A2aTask> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);
        let poll_interval = Duration::from_millis(self.poll_interval_ms);

        loop {
            let task = self.get_task(task_id).await?;

            if task.state.is_terminal() {
                return Ok(task);
            }

            if start.elapsed() > timeout {
                return Err(A2aError::TaskTimeout {
                    task_id: task_id.to_string(),
                    seconds: self.timeout_secs,
                });
            }

            tracing::trace!(
                agent = %self.name,
                task_id = %task_id,
                state = ?task.state,
                "polling A2A task"
            );

            sleep(poll_interval).await;
        }
    }
}

#[async_trait]
impl A2aClient for HttpA2aClient {
    async fn fetch_card(&mut self) -> Result<AgentCard> {
        tracing::debug!(
            agent = %self.name,
            url = %self.card_url,
            "fetching agent card"
        );

        let response = self
            .http
            .get(&self.card_url)
            .send()
            .await
            .map_err(|e| A2aError::CardFetchFailed {
                agent: self.name.clone(),
                reason: e.to_string(),
            })?;

        if !response.status().is_success() {
            return Err(A2aError::CardFetchFailed {
                agent: self.name.clone(),
                reason: format!("HTTP {}", response.status()),
            });
        }

        let card: AgentCard = response.json().await.map_err(|e| A2aError::CardFetchFailed {
            agent: self.name.clone(),
            reason: format!("invalid JSON: {}", e),
        })?;

        tracing::info!(
            agent = %self.name,
            card_name = %card.name,
            skills = %card.skills.len(),
            "loaded agent card"
        );

        self.cached_card = Some(card.clone());
        Ok(card)
    }

    fn card(&self) -> Option<&AgentCard> {
        self.cached_card.as_ref()
    }

    async fn execute_task(&self, skill: &str, input: serde_json::Value) -> Result<TaskResult> {
        // Verify skill exists
        if let Some(card) = &self.cached_card {
            if card.skill(skill).is_none() {
                return Err(A2aError::SkillNotFound {
                    agent: self.name.clone(),
                    skill: skill.to_string(),
                });
            }
        }

        // Send the message to start the task
        let task_id = self.send_message(skill, input).await?;

        tracing::debug!(
            agent = %self.name,
            skill = %skill,
            task_id = %task_id,
            "started A2A task"
        );

        // Poll until completion
        let task = self.poll_until_complete(&task_id).await?;

        match task.state {
            TaskState::Completed => {
                task.result.ok_or_else(|| A2aError::Protocol {
                    reason: "completed task missing result".into(),
                })
            }
            TaskState::Failed => Err(A2aError::TaskFailed {
                task_id,
                reason: task.error.unwrap_or_else(|| "unknown error".to_string()),
            }),
            TaskState::Cancelled => Err(A2aError::TaskCancelled { task_id }),
            _ => Err(A2aError::Protocol {
                reason: format!("unexpected terminal state: {:?}", task.state),
            }),
        }
    }

    async fn send_message(&self, skill: &str, input: serde_json::Value) -> Result<String> {
        let params = serde_json::json!({
            "skill": skill,
            "message": {
                "parts": [{
                    "type": "text",
                    "text": serde_json::to_string(&input).unwrap_or_default()
                }]
            }
        });

        #[derive(serde::Deserialize)]
        struct SendMessageResult {
            #[serde(rename = "taskId")]
            task_id: String,
        }

        let result: SendMessageResult = self.rpc("tasks/send", Some(params)).await?;
        Ok(result.task_id)
    }

    async fn get_task(&self, task_id: &str) -> Result<A2aTask> {
        let params = serde_json::json!({
            "taskId": task_id
        });

        self.rpc("tasks/get", Some(params)).await
    }

    async fn cancel_task(&self, task_id: &str) -> Result<()> {
        let params = serde_json::json!({
            "taskId": task_id
        });

        let _: serde_json::Value = self.rpc("tasks/cancel", Some(params)).await?;
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_card_url() {
        let client = HttpA2aClient::new(HttpA2aConfig {
            name: "test".to_string(),
            url: "https://example.com/api".to_string(),
            card_url: None,
            bearer_token: None,
            timeout_secs: 300,
            poll_interval_ms: 1000,
        })
        .unwrap();

        assert_eq!(
            client.card_url,
            "https://example.com/api/.well-known/agent.json"
        );
    }

    #[test]
    fn custom_card_url() {
        let client = HttpA2aClient::new(HttpA2aConfig {
            name: "test".to_string(),
            url: "https://example.com/api".to_string(),
            card_url: Some("https://other.com/card.json".to_string()),
            bearer_token: None,
            timeout_secs: 300,
            poll_interval_ms: 1000,
        })
        .unwrap();

        assert_eq!(client.card_url, "https://other.com/card.json");
    }
}
