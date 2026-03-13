//! Event sources — multiple input channels for task execution.
//!
//! Servitor can receive tasks from multiple sources:
//! - Cron: scheduled tasks
//! - SSE: egregore feed subscription
//! - MCP notifications: server-pushed events
//! - Hook: stdin (for egregore hook mode)
//! - Direct: CLI exec command

pub mod cron;
pub mod mcp;
pub mod sse;

use async_trait::async_trait;
use std::collections::HashMap;

use crate::egregore::Task;

/// Trait for event sources that yield tasks.
#[async_trait]
pub trait EventSource: Send {
    /// Get the next task from this source.
    /// Returns None if no task is currently available.
    async fn next(&mut self) -> Option<Task>;

    /// Get the source name (for logging).
    fn name(&self) -> &str;
}

/// Routes events from multiple sources to a task executor.
pub struct EventRouter {
    sources: Vec<Box<dyn EventSource>>,
}

impl EventRouter {
    /// Create a new event router.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Add an event source.
    pub fn add_source(&mut self, source: Box<dyn EventSource>) {
        self.sources.push(source);
    }

    /// Poll all sources and return the first available task.
    pub async fn poll(&mut self) -> Option<(usize, Task)> {
        for (idx, source) in self.sources.iter_mut().enumerate() {
            if let Some(task) = source.next().await {
                tracing::debug!(source = source.name(), hash = %task.hash, "task from event source");
                return Some((idx, task));
            }
        }
        None
    }

    /// Get the number of registered sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
}

impl Default for EventRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a task from a scheduled task definition.
pub fn task_from_schedule(
    name: &str,
    prompt: &str,
    context: HashMap<String, serde_json::Value>,
) -> Task {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(prompt.as_bytes());
    hasher.update(chrono::Utc::now().timestamp().to_le_bytes());
    let hash = hasher.finalize();
    let hash_str: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    Task {
        msg_type: "task".to_string(),
        hash: hash_str,
        prompt: prompt.to_string(),
        required_caps: vec![],
        parent_id: None,
        context,
        scope_override: None,
        priority: 0,
        timeout_secs: None,
        author: None,
        keeper: None,
    }
}
