//! SSE event source — egregore feed subscription.
//!
//! Authorization is handled by the Authority system in the main event loop,
//! not here. This source just delivers tasks with author info attached.

use std::collections::HashSet;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource as ReqwestEventSource};

use crate::egregore::{EgregoreMessage, Task};
use crate::error::{Result, ServitorError};
use crate::events::EventSource;

/// SSE-based event source for egregore feed subscription.
pub struct SseSource {
    api_url: String,
    capabilities: HashSet<String>,
    event_source: Option<ReqwestEventSource>,
    connected: bool,
}

impl SseSource {
    /// Create a new SSE source.
    pub fn new(api_url: &str, capabilities: Vec<String>) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            capabilities: capabilities.into_iter().collect(),
            event_source: None,
            connected: false,
        }
    }

    /// Connect to the SSE endpoint.
    pub fn connect(&mut self) -> Result<()> {
        let url = format!("{}/v1/events", self.api_url);
        tracing::info!(url = %url, "connecting to egregore SSE");

        let client = reqwest::Client::new();
        let request = client.get(&url);
        let event_source = ReqwestEventSource::new(request).map_err(|e| ServitorError::Sse {
            reason: format!("failed to create SSE connection: {}", e),
        })?;

        self.event_source = Some(event_source);
        self.connected = true;
        Ok(())
    }

    /// Check if a task matches our capabilities.
    fn matches_capabilities(&self, task: &Task) -> bool {
        // If task has no required caps, accept it
        if task.required_caps.is_empty() {
            return true;
        }

        // Check if we have all required capabilities
        task.required_caps
            .iter()
            .all(|cap| self.capabilities.contains(cap))
    }

    /// Process an SSE event.
    fn process_event(&mut self, event: &Event) -> Option<EgregoreMessage> {
        match event {
            Event::Open => {
                tracing::info!("SSE connection established");
                self.connected = true;
                None
            }
            Event::Message(msg) => {
                match serde_json::from_str::<EgregoreMessage>(&msg.data) {
                    Ok(message) => {
                        if let Some(task) = message.as_task() {
                            if self.matches_capabilities(&task) {
                                return Some(message);
                            }

                            tracing::trace!(
                                hash = %task.hash,
                                required_caps = ?task.required_caps,
                                "skipping task (capability mismatch)"
                            );
                            return None;
                        }

                        if matches!(message.content_type(), Some("task_assign" | "task_ping")) {
                            return Some(message);
                        }
                    }
                    Err(e) => {
                        tracing::trace!(error = %e, "failed to parse SSE message");
                    }
                }
                None
            }
        }
    }
}

#[async_trait]
impl EventSource for SseSource {
    async fn next(&mut self) -> Option<Task> {
        while let Some(message) = self.next_message().await {
            if let Some(mut task) = message.as_task() {
                task.author = Some(message.author.0.clone());
                task.normalize(Some(&message.author));

                tracing::debug!(
                    hash = %task.hash,
                    author = %message.author.0,
                    prompt = %task.prompt,
                    "received matching task from SSE"
                );
                return Some(task);
            }
        }

        None
    }

    fn name(&self) -> &str {
        "sse"
    }
}

impl SseSource {
    /// Poll one raw egregore message from SSE.
    pub async fn next_message(&mut self) -> Option<EgregoreMessage> {
        if self.event_source.is_none() {
            if let Err(e) = self.connect() {
                tracing::warn!(error = %e, "failed to connect to SSE");
                return None;
            }
        }

        if let Some(ref mut es) = self.event_source {
            match tokio::time::timeout(std::time::Duration::from_millis(100), es.next()).await {
                Ok(Some(Ok(event))) => {
                    return self.process_event(&event);
                }
                Ok(Some(Err(e))) => {
                    tracing::warn!(error = %e, "SSE error, will reconnect");
                    self.event_source = None;
                    self.connected = false;
                }
                Ok(None) => {
                    tracing::info!("SSE stream ended, will reconnect");
                    self.event_source = None;
                    self.connected = false;
                }
                Err(_) => {}
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_matching() {
        let source = SseSource::new("http://localhost:7654", vec!["shell".to_string()]);

        // Task with no required caps matches
        let task1 = Task {
            msg_type: "task".to_string(),
            id: None,
            hash: "abc".to_string(),
            task_type: None,
            request: None,
            requestor: None,
            prompt: "test".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: Default::default(),
            scope_override: None,
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
            tool_calls: vec![],
            depends_on: vec![],
        };
        assert!(source.matches_capabilities(&task1));

        // Task requiring shell matches
        let task2 = Task {
            msg_type: "task".to_string(),
            id: None,
            hash: "def".to_string(),
            task_type: None,
            request: None,
            requestor: None,
            prompt: "test".to_string(),
            required_caps: vec!["shell".to_string()],
            parent_id: None,
            context: Default::default(),
            scope_override: None,
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
            tool_calls: vec![],
            depends_on: vec![],
        };
        assert!(source.matches_capabilities(&task2));

        // Task requiring docker doesn't match
        let task3 = Task {
            msg_type: "task".to_string(),
            id: None,
            hash: "ghi".to_string(),
            task_type: None,
            request: None,
            requestor: None,
            prompt: "test".to_string(),
            required_caps: vec!["docker".to_string()],
            parent_id: None,
            context: Default::default(),
            scope_override: None,
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
            tool_calls: vec![],
            depends_on: vec![],
        };
        assert!(!source.matches_capabilities(&task3));
    }
}
