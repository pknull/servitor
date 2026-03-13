//! SSE event source — egregore feed subscription.
//!
//! Authorization is handled by the Authority system in the main event loop,
//! not here. This source just delivers tasks with author info attached.

use chrono::Utc;
use std::collections::HashSet;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource as ReqwestEventSource};
use serde::Deserialize;

use crate::egregore::{EgregoreMessage, ServitorProfile, Task};
use crate::error::{Result, ServitorError};
use crate::events::EventSource;
use crate::group::ConsumerGroupCoordinator;

/// SSE-based event source for egregore feed subscription.
pub struct SseSource {
    api_url: String,
    capabilities: HashSet<String>,
    consumer_group: Option<ConsumerGroupCoordinator>,
    event_source: Option<ReqwestEventSource>,
    pending_task: Option<Task>,
    connected: bool,
}

impl SseSource {
    /// Create a new SSE source.
    pub fn new(api_url: &str, capabilities: Vec<String>) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            capabilities: capabilities.into_iter().collect(),
            consumer_group: None,
            event_source: None,
            pending_task: None,
            connected: false,
        }
    }

    /// Enable deterministic task ownership for a named consumer group.
    pub fn with_consumer_group(mut self, consumer_group: ConsumerGroupCoordinator) -> Self {
        self.consumer_group = Some(consumer_group);
        self
    }

    /// Connect to the SSE endpoint.
    pub async fn connect(&mut self) -> Result<()> {
        let url = format!("{}/v1/events", self.api_url);
        tracing::info!(url = %url, "connecting to egregore SSE");

        let client = reqwest::Client::new();
        let request = client.get(&url);
        let event_source = ReqwestEventSource::new(request).map_err(|e| ServitorError::Sse {
            reason: format!("failed to create SSE connection: {}", e),
        })?;

        self.event_source = Some(event_source);
        self.connected = true;

        if let Some(group_name) = self
            .consumer_group
            .as_ref()
            .map(|consumer_group| consumer_group.group_name().to_string())
        {
            match self.bootstrap_group_membership().await {
                Ok(count) => {
                    tracing::debug!(group = %group_name, members = count, "bootstrapped consumer group membership");
                }
                Err(error) => {
                    tracing::warn!(error = %error, group = %group_name, "failed to bootstrap consumer group membership");
                }
            }
        }

        Ok(())
    }

    async fn bootstrap_group_membership(&mut self) -> Result<usize> {
        let Some(group) = self.consumer_group.as_mut() else {
            return Ok(0);
        };

        let url = format!(
            "{}/v1/feed?content_type=servitor_profile&include_self=true&limit=200",
            self.api_url
        );
        let response = reqwest::get(&url).await.map_err(|e| ServitorError::Sse {
            reason: format!("failed to fetch recent servitor profiles: {}", e),
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Sse {
                reason: format!("bootstrap profile fetch failed with {}: {}", status, body),
            });
        }

        let wrapper: FeedResponse = response.json().await.map_err(|e| ServitorError::Sse {
            reason: format!("failed to parse feed bootstrap response: {}", e),
        })?;

        for message in wrapper.data.unwrap_or_default() {
            if let Some(profile) = message.as_servitor_profile() {
                group.observe_profile(&profile, message.timestamp);
            }
        }

        Ok(group.active_members(Utc::now()).len())
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
    fn process_event(&mut self, event: &Event) -> Option<Task> {
        match event {
            Event::Open => {
                tracing::info!("SSE connection established");
                self.connected = true;
                None
            }
            Event::Message(msg) => {
                // Try to parse as egregore message
                match serde_json::from_str::<EgregoreMessage>(&msg.data) {
                    Ok(message) => {
                        if let Some(profile) = message.as_servitor_profile() {
                            self.observe_profile(&profile, message.timestamp);
                            return None;
                        }

                        // Check if it's a task (authorization handled by Authority in main loop)
                        if let Some(mut task) = message.as_task() {
                            if self.matches_capabilities(&task) {
                                if !self.owns_task(&task.hash) {
                                    tracing::trace!(
                                        hash = %task.hash,
                                        "skipping task (owned by another consumer group member)"
                                    );
                                    return None;
                                }

                                // Attach author for authorization check in main loop
                                task.author = Some(message.author.0.clone());

                                tracing::debug!(
                                    hash = %task.hash,
                                    author = %message.author.0,
                                    prompt = %task.prompt,
                                    "received matching task from SSE"
                                );
                                return Some(task);
                            } else {
                                tracing::trace!(
                                    hash = %task.hash,
                                    required_caps = ?task.required_caps,
                                    "skipping task (capability mismatch)"
                                );
                            }
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

    fn observe_profile(&mut self, profile: &ServitorProfile, seen_at: chrono::DateTime<Utc>) {
        if let Some(consumer_group) = self.consumer_group.as_mut() {
            consumer_group.observe_profile(profile, seen_at);
        }
    }

    fn owns_task(&mut self, task_hash: &str) -> bool {
        match self.consumer_group.as_mut() {
            Some(consumer_group) => consumer_group.should_process(task_hash, Utc::now()),
            None => true,
        }
    }
}

#[async_trait]
impl EventSource for SseSource {
    async fn next(&mut self) -> Option<Task> {
        // Return pending task if we have one
        if let Some(task) = self.pending_task.take() {
            return Some(task);
        }

        // Ensure we're connected
        if self.event_source.is_none() {
            if let Err(e) = self.connect().await {
                tracing::warn!(error = %e, "failed to connect to SSE");
                return None;
            }
        }

        // Poll the event source
        if let Some(ref mut es) = self.event_source {
            // Try to get one event without blocking too long
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
                    // Stream ended
                    tracing::info!("SSE stream ended, will reconnect");
                    self.event_source = None;
                    self.connected = false;
                }
                Err(_) => {
                    // Timeout, no events available
                }
            }
        }

        None
    }

    fn name(&self) -> &str {
        "sse"
    }
}

#[derive(Debug, Deserialize)]
struct FeedResponse {
    #[allow(dead_code)]
    success: bool,
    data: Option<Vec<EgregoreMessage>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::ConsumerGroupCoordinator;
    use crate::identity::PublicId;

    fn profile_message(id: &str, groups: &[&str]) -> Event {
        let mut profile = ServitorProfile::new(PublicId(id.to_string()), 10_000);
        profile.groups = groups.iter().map(|group| group.to_string()).collect();

        Event::Message(eventsource_stream::Event {
            event: "message".to_string(),
            data: serde_json::json!({
                "author": id,
                "sequence": 1,
                "timestamp": Utc::now(),
                "content": profile,
                "hash": format!("profile-{id}"),
                "signature": "sig",
                "tags": ["servitor_profile"]
            })
            .to_string(),
            id: "".to_string(),
            retry: None,
        })
    }

    fn task_message(hash: &str) -> Event {
        Event::Message(eventsource_stream::Event {
            event: "message".to_string(),
            data: serde_json::json!({
                "author": "@author.ed25519",
                "sequence": 1,
                "timestamp": Utc::now(),
                "content": {
                    "type": "task",
                    "hash": hash,
                    "prompt": "test",
                    "required_caps": ["shell"]
                },
                "hash": format!("message-{hash}"),
                "signature": "sig",
                "tags": ["task"]
            })
            .to_string(),
            id: "".to_string(),
            retry: None,
        })
    }

    #[test]
    fn capability_matching() {
        let source = SseSource::new("http://localhost:7654", vec!["shell".to_string()]);

        // Task with no required caps matches
        let task1 = Task {
            msg_type: "task".to_string(),
            hash: "abc".to_string(),
            prompt: "test".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };
        assert!(source.matches_capabilities(&task1));

        // Task requiring shell matches
        let task2 = Task {
            msg_type: "task".to_string(),
            hash: "def".to_string(),
            prompt: "test".to_string(),
            required_caps: vec!["shell".to_string()],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };
        assert!(source.matches_capabilities(&task2));

        // Task requiring docker doesn't match
        let task3 = Task {
            msg_type: "task".to_string(),
            hash: "ghi".to_string(),
            prompt: "test".to_string(),
            required_caps: vec!["docker".to_string()],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };
        assert!(!source.matches_capabilities(&task3));
    }

    #[test]
    fn consumer_group_filters_tasks_to_local_owner() {
        let mut coordinator =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));
        let local_profile = profile_message("@self.ed25519", &["workers"]);
        let peer_profile = profile_message("@peer.ed25519", &["workers"]);
        let now = Utc::now();

        let mut local_membership =
            ServitorProfile::new(PublicId("@self.ed25519".to_string()), 10_000);
        local_membership.groups = vec!["workers".to_string()];
        coordinator.observe_profile(&local_membership, now);

        let mut peer_membership =
            ServitorProfile::new(PublicId("@peer.ed25519".to_string()), 10_000);
        peer_membership.groups = vec!["workers".to_string()];
        coordinator.observe_profile(&peer_membership, now);

        let mut source = SseSource::new("http://localhost:7654", vec!["shell".to_string()])
            .with_consumer_group(coordinator.clone());

        let _ = source.process_event(&local_profile);
        let _ = source.process_event(&peer_profile);

        let task_hash = (0..128)
            .map(|idx| format!("task-{idx}"))
            .find(|hash| {
                coordinator.owner_for(hash, Utc::now())
                    == Some(PublicId("@self.ed25519".to_string()))
            })
            .expect("expected at least one local task hash");

        assert!(source.process_event(&task_message(&task_hash)).is_some());
    }

    #[test]
    fn consumer_group_skips_tasks_owned_by_peer() {
        let mut coordinator =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));
        let local_profile = profile_message("@self.ed25519", &["workers"]);
        let peer_profile = profile_message("@peer.ed25519", &["workers"]);
        let now = Utc::now();

        let mut local_membership =
            ServitorProfile::new(PublicId("@self.ed25519".to_string()), 10_000);
        local_membership.groups = vec!["workers".to_string()];
        coordinator.observe_profile(&local_membership, now);

        let mut peer_membership =
            ServitorProfile::new(PublicId("@peer.ed25519".to_string()), 10_000);
        peer_membership.groups = vec!["workers".to_string()];
        coordinator.observe_profile(&peer_membership, now);

        let mut source = SseSource::new("http://localhost:7654", vec!["shell".to_string()])
            .with_consumer_group(coordinator.clone());

        let _ = source.process_event(&local_profile);
        let _ = source.process_event(&peer_profile);

        let task_hash = (0..128)
            .map(|idx| format!("task-{idx}"))
            .find(|hash| {
                coordinator.owner_for(hash, Utc::now())
                    == Some(PublicId("@peer.ed25519".to_string()))
            })
            .expect("expected at least one peer-owned task hash");

        assert!(source.process_event(&task_message(&task_hash)).is_none());
    }
}
