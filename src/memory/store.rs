//! Memory store trait — abstraction over local storage backends.

use std::any::Any;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::egregore::messages::{Task, TaskResult};
use crate::error::Result;

/// A task queued for local execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTask {
    /// Task hash (primary key).
    pub hash: String,
    /// Task prompt.
    pub prompt: String,
    /// Parent task hash (for threading).
    pub parent_id: Option<String>,
    /// Priority (higher = more urgent).
    pub priority: i32,
    /// Additional context as JSON.
    pub context: serde_json::Value,
    /// Author identity (egregore pubkey or other ID).
    pub author: Option<String>,
    /// When the task was queued.
    pub created_at: DateTime<Utc>,
    /// Task status: pending, in_progress, completed.
    pub status: String,
}

impl From<&Task> for StoredTask {
    fn from(task: &Task) -> Self {
        Self {
            hash: task.hash.clone(),
            prompt: task.prompt.clone(),
            parent_id: task.parent_id.clone(),
            priority: task.priority,
            context: serde_json::to_value(&task.context).unwrap_or_default(),
            author: task.author.clone(),
            created_at: Utc::now(),
            status: "pending".to_string(),
        }
    }
}

impl StoredTask {
    /// Convert back to a Task for execution.
    pub fn to_task(&self) -> Task {
        Task {
            msg_type: "task".to_string(),
            hash: self.hash.clone(),
            prompt: self.prompt.clone(),
            required_caps: vec![],
            parent_id: self.parent_id.clone(),
            context: serde_json::from_value(self.context.clone()).unwrap_or_default(),
            priority: self.priority,
            timeout_secs: None,
            author: self.author.clone(),
            keeper: None,
        }
    }
}

/// A result stored locally, pending sync to egregore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredResult {
    /// Result hash (primary key).
    pub result_hash: String,
    /// Task hash this result is for.
    pub task_hash: String,
    /// Execution status.
    pub status: String,
    /// Result content as JSON.
    pub result: Option<serde_json::Value>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Signed attestation as JSON.
    pub attestation: serde_json::Value,
    /// When the result was created.
    pub created_at: DateTime<Utc>,
    /// When synced to egregore (None = pending sync).
    pub synced_at: Option<DateTime<Utc>>,
}

impl From<&TaskResult> for StoredResult {
    fn from(result: &TaskResult) -> Self {
        Self {
            result_hash: result.result_hash.clone(),
            task_hash: result.task_hash.clone(),
            status: format!("{:?}", result.status).to_lowercase(),
            result: result.result.clone(),
            error: result.error.clone(),
            attestation: serde_json::to_value(&result.attestation).unwrap_or_default(),
            created_at: Utc::now(),
            synced_at: None,
        }
    }
}

/// A conversation turn for history caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Message hash.
    pub hash: String,
    /// Role: "user" or "assistant".
    pub role: String,
    /// Message content.
    pub content: String,
    /// Parent message hash (for threading).
    pub parent_hash: Option<String>,
    /// When cached.
    pub cached_at: DateTime<Utc>,
    /// Cache expiration.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Memory store trait — abstraction over storage backends.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Queue a task for local execution.
    async fn queue_task(&self, task: &Task) -> Result<()>;

    /// Get the next pending task (ordered by priority, then created_at).
    async fn next_task(&self) -> Result<Option<StoredTask>>;

    /// Mark a task as in_progress.
    async fn claim_task(&self, hash: &str) -> Result<()>;

    /// Store a task result.
    async fn store_result(&self, result: &TaskResult) -> Result<()>;

    /// Get results pending sync to egregore.
    async fn pending_results(&self) -> Result<Vec<StoredResult>>;

    /// Mark a result as synced.
    async fn mark_synced(&self, result_hash: &str) -> Result<()>;

    /// Fetch conversation history for a parent_id.
    async fn fetch_history(&self, parent_id: &str) -> Result<Vec<ConversationTurn>>;

    /// Cache conversation turns.
    async fn cache_history(&self, turns: &[ConversationTurn]) -> Result<()>;

    /// Clear expired cache entries.
    async fn clear_expired_cache(&self) -> Result<u64>;

    /// Downcast to concrete type for specialized operations.
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn stored_task_roundtrip() {
        let task = Task {
            msg_type: "task".to_string(),
            hash: "abc123".to_string(),
            prompt: "test prompt".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: HashMap::new(),
            priority: 5,
            timeout_secs: None,
            author: Some("author123".to_string()),
            keeper: None,
        };

        let stored = StoredTask::from(&task);
        let back = stored.to_task();

        assert_eq!(back.hash, task.hash);
        assert_eq!(back.prompt, task.prompt);
        assert_eq!(back.priority, task.priority);
        assert_eq!(back.author, task.author);
    }
}
