//! Context fetching — query egregore feed for conversation history.
//!
//! Fetches messages by hash and builds conversation threads from parent_id chains.

use crate::egregore::messages::EgregoreMessage;
use crate::egregore::publish::EgregoreClient;
use crate::error::{Result, ServitorError};

impl EgregoreClient {
    /// Fetch a single message by hash.
    pub async fn fetch_message(&self, hash: &str) -> Result<Option<EgregoreMessage>> {
        let url = format!("{}/v1/messages/{}", self.api_url(), hash);

        let response = reqwest::get(&url).await.map_err(|e| ServitorError::Egregore {
            reason: format!("fetch message request failed: {}", e),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Egregore {
                reason: format!("fetch message failed with {}: {}", status, body),
            });
        }

        let wrapper: MessageWrapper =
            response
                .json()
                .await
                .map_err(|e| ServitorError::Egregore {
                    reason: format!("failed to parse message response: {}", e),
                })?;

        Ok(Some(wrapper.message))
    }

    /// Fetch a thread (message + ancestors via parent_id).
    ///
    /// Returns messages in chronological order (oldest first).
    pub async fn fetch_thread(&self, hash: &str) -> Result<Vec<EgregoreMessage>> {
        let url = format!("{}/v1/thread/{}", self.api_url(), hash);

        let response = reqwest::get(&url).await.map_err(|e| ServitorError::Egregore {
            reason: format!("fetch thread request failed: {}", e),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // No thread found, try fetching just the message
            if let Some(msg) = self.fetch_message(hash).await? {
                return Ok(vec![msg]);
            }
            return Ok(vec![]);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Egregore {
                reason: format!("fetch thread failed with {}: {}", status, body),
            });
        }

        let wrapper: ThreadWrapper =
            response
                .json()
                .await
                .map_err(|e| ServitorError::Egregore {
                    reason: format!("failed to parse thread response: {}", e),
                })?;

        Ok(wrapper.messages)
    }

    /// Build conversation history from a thread.
    ///
    /// Extracts prompts and responses from messages, suitable for LLM context.
    pub async fn fetch_conversation_history(
        &self,
        hash: &str,
    ) -> Result<Vec<ConversationTurn>> {
        let messages = self.fetch_thread(hash).await?;
        let mut turns = Vec::new();

        for msg in messages {
            if let Some(prompt) = msg.prompt() {
                turns.push(ConversationTurn {
                    role: "user".to_string(),
                    content: prompt.to_string(),
                    hash: msg.hash.clone(),
                });
            }

            // Check for task_result content
            if let Some(result) = msg.as_task_result() {
                if let Some(ref r) = result.result {
                    if let Some(response) = r.get("response").and_then(|v| v.as_str()) {
                        turns.push(ConversationTurn {
                            role: "assistant".to_string(),
                            content: response.to_string(),
                            hash: msg.hash.clone(),
                        });
                    }
                }
            }
        }

        Ok(turns)
    }
}

/// Wrapper for single message response.
#[derive(Debug, serde::Deserialize)]
struct MessageWrapper {
    message: EgregoreMessage,
}

/// Wrapper for thread response.
#[derive(Debug, serde::Deserialize)]
struct ThreadWrapper {
    messages: Vec<EgregoreMessage>,
}

/// A turn in a conversation (for LLM context).
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_turn_creation() {
        let turn = ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            hash: "abc123".to_string(),
        };
        assert_eq!(turn.role, "user");
        assert_eq!(turn.content, "Hello");
    }
}
