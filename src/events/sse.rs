//! SSE event source — egregore feed subscription.
//!
//! Authorization is handled by the Authority system in the main event loop,
//! not here. This source just delivers tasks with author info attached.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource as ReqwestEventSource};

use crate::egregore::{EgregoreMessage, Task};
use crate::error::{Result, ServitorError};
use crate::events::EventSource;

/// First wait after a failed connection; it doubles per consecutive failure.
const RECONNECT_INITIAL: Duration = Duration::from_secs(1);

/// Longest wait between reconnect attempts. Egregore's SSE stream carries
/// only new messages, so anything published while disconnected is missed;
/// the cap keeps that gap short.
const RECONNECT_MAX: Duration = Duration::from_secs(15);

/// A connection that delivers data, or stays open this long, was healthy:
/// its end starts the backoff afresh.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);

/// Bound on establishing a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP keepalive interval, so a silently dead peer is noticed.
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Wait before the next attempt after `failures` consecutive failures.
fn reconnect_delay(failures: u32) -> Duration {
    // 2^4 seconds already exceeds the cap, so larger exponents never matter.
    let doublings = failures.saturating_sub(1).min(4);
    (RECONNECT_INITIAL * 2u32.pow(doublings)).min(RECONNECT_MAX)
}

/// SSE-based event source for egregore feed subscription.
pub struct SseSource {
    api_url: String,
    capabilities: HashSet<String>,
    event_source: Option<ReqwestEventSource>,
    connected: bool,
    /// Consecutive failed or ended connections since the last healthy one.
    failures: u32,
    /// No reconnect is attempted before this instant.
    next_attempt_at: Option<Instant>,
    /// When the current connection opened.
    opened_at: Option<Instant>,
}

impl SseSource {
    /// Create a new SSE source.
    pub fn new(api_url: &str, capabilities: Vec<String>) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            capabilities: capabilities.into_iter().collect(),
            event_source: None,
            connected: false,
            failures: 0,
            next_attempt_at: None,
            opened_at: None,
        }
    }

    /// Connect to the SSE endpoint.
    pub fn connect(&mut self) -> Result<()> {
        let url = format!("{}/v1/events", self.api_url);
        if self.failures == 0 {
            tracing::info!(url = %url, "connecting to egregore SSE");
        } else {
            tracing::debug!(url = %url, failures = self.failures, "reconnecting to egregore SSE");
        }

        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .tcp_keepalive(TCP_KEEPALIVE)
            .build()
            .map_err(|e| ServitorError::Sse {
                reason: format!("failed to build SSE client: {}", e.without_url()),
            })?;
        let request = client.get(&url);
        let mut event_source =
            ReqwestEventSource::new(request).map_err(|e| ServitorError::Sse {
                reason: format!("failed to create SSE connection: {}", e),
            })?;
        // This source owns reconnection and its backoff.
        event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

        self.event_source = Some(event_source);
        self.connected = true;
        Ok(())
    }

    /// Drop the connection and schedule the next attempt, doubling the wait
    /// with each consecutive failure. Returns the wait.
    fn schedule_reconnect(&mut self) -> Duration {
        self.event_source = None;
        self.connected = false;
        if self
            .opened_at
            .take()
            .is_some_and(|opened| opened.elapsed() >= HEALTHY_AFTER)
        {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        let delay = reconnect_delay(self.failures);
        self.next_attempt_at = Some(Instant::now() + delay);
        delay
    }

    /// Log a connection failure: the first at WARN, repeats at DEBUG.
    fn log_failure(&self, error: &dyn std::fmt::Display, retry_in: Duration) {
        if self.failures == 1 {
            tracing::warn!(error = %error, retry_in = ?retry_in, "SSE error, will reconnect");
        } else {
            tracing::debug!(
                error = %error,
                failures = self.failures,
                retry_in = ?retry_in,
                "SSE error, will reconnect"
            );
        }
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
                // Headers alone don't prove a healthy stream: a server can
                // send them and close at once.
                if self.failures == 0 {
                    tracing::info!("SSE connection established");
                } else {
                    tracing::debug!(failures = self.failures, "SSE connection opened");
                }
                self.connected = true;
                self.opened_at = Some(Instant::now());
                None
            }
            Event::Message(msg) => {
                if self.failures > 0 {
                    tracing::info!(after_failures = self.failures, "SSE connection restored");
                    self.failures = 0;
                    self.next_attempt_at = None;
                }
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

                        if matches!(
                            message.content_type(),
                            Some("task_assign" | "assign_task" | "task_ping")
                        ) {
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
            if self.next_attempt_at.is_some_and(|at| Instant::now() < at) {
                return None;
            }
            if let Err(e) = self.connect() {
                let delay = self.schedule_reconnect();
                self.log_failure(&e, delay);
                return None;
            }
        }

        if let Some(ref mut es) = self.event_source {
            match tokio::time::timeout(std::time::Duration::from_millis(100), es.next()).await {
                Ok(Some(Ok(event))) => {
                    return self.process_event(&event);
                }
                Ok(Some(Err(e))) => {
                    let delay = self.schedule_reconnect();
                    self.log_failure(&e, delay);
                }
                Ok(None) => {
                    let delay = self.schedule_reconnect();
                    self.log_failure(&"stream ended", delay);
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

    #[tokio::test]
    async fn failed_connections_back_off_instead_of_retrying_every_poll() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Accept and drop every connection, counting the attempts.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });

        let mut source = SseSource::new(&format!("http://{addr}"), vec![]);
        // The first poll connects and observes the failure.
        for _ in 0..20 {
            assert!(source.next_message().await.is_none());
            if source.failures == 1 {
                break;
            }
        }
        assert_eq!(source.failures, 1);
        let retry_at = source.next_attempt_at.unwrap();
        assert!(retry_at > Instant::now() + Duration::from_millis(500));

        // Polls inside the wait make no further attempt.
        for _ in 0..4 {
            assert!(source.next_message().await.is_none());
        }
        assert!(Instant::now() < retry_at, "test outlived the wait");
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }

    /// Data on the stream proves the connection healthy and clears the
    /// failure history.
    #[tokio::test]
    async fn data_on_the_stream_resets_the_backoff() {
        let addr = sse_server(b"data: {}\n\n", true).await;
        let mut source = SseSource::new(&format!("http://{addr}"), vec![]);
        source.failures = 3;
        // The wait has passed, so the next poll reconnects.
        source.next_attempt_at = Some(Instant::now());
        for _ in 0..20 {
            source.next_message().await;
            if source.failures == 0 {
                break;
            }
        }
        assert_eq!(source.failures, 0);
        assert!(source.next_attempt_at.is_none());
        assert!(source.event_source.is_some());
    }

    /// Serve every request with SSE headers followed by `body`, then hold or
    /// close the connection.
    async fn sse_server(body: &'static [u8], hold: bool) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
                        .await;
                    let _ = stream.write_all(body).await;
                    if hold {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });
        addr
    }

    /// A server that answers with headers and then closes is not healthy, so
    /// each cycle counts as another failure and the wait keeps growing.
    #[tokio::test]
    async fn a_flapping_server_keeps_backing_off() {
        let addr = sse_server(b"", false).await;
        let mut source = SseSource::new(&format!("http://{addr}"), vec![]);

        for cycle in 1..=3 {
            // Skip the wait itself; the test is about the failure count.
            source.next_attempt_at = Some(Instant::now());
            for _ in 0..20 {
                source.next_message().await;
                if source.failures >= cycle {
                    break;
                }
            }
            assert_eq!(source.failures, cycle);
        }
        assert!(source.next_attempt_at.unwrap() > Instant::now() + Duration::from_secs(3));
    }

    #[test]
    fn reconnect_delay_doubles_to_a_cap() {
        assert_eq!(reconnect_delay(1), Duration::from_secs(1));
        assert_eq!(reconnect_delay(2), Duration::from_secs(2));
        assert_eq!(reconnect_delay(4), Duration::from_secs(8));
        assert_eq!(reconnect_delay(5), Duration::from_secs(15));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(15));
    }
}
