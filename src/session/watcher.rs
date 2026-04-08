//! Egregore watcher for task completion notifications.
//!
//! Monitors the egregore feed for `task_result` messages that match
//! pending task correlations, then notifies the appropriate session.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::interval;

use super::store::SessionStore;
use super::types::PendingTask;
use crate::egregore::{EgregoreClient, EgregoreMessage};
use crate::error::Result;

/// Event emitted when a delegated task completes.
#[derive(Debug, Clone)]
pub struct TaskCompletionEvent {
    /// The pending task that was completed.
    pub task: PendingTask,

    /// The task result message from egregore.
    pub result: TaskResultInfo,
}

/// Information about the completed task result.
#[derive(Debug, Clone)]
pub struct TaskResultInfo {
    /// Hash of the result message.
    pub hash: String,

    /// Success status.
    pub success: bool,

    /// Result summary.
    pub summary: String,

    /// Full result content (if available).
    pub content: Option<String>,
}

/// Watches egregore for task completion events.
pub struct TaskWatcher {
    egregore: EgregoreClient,
    poll_interval: Duration,
}

impl TaskWatcher {
    /// Create a new task watcher.
    pub fn new(egregore: EgregoreClient) -> Self {
        Self {
            egregore,
            poll_interval: Duration::from_secs(5),
        }
    }

    /// Set the polling interval.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Start watching for task completions.
    ///
    /// Returns a channel that receives completion events.
    pub fn start(
        self,
        store: Arc<SessionStore>,
    ) -> (mpsc::Receiver<TaskCompletionEvent>, TaskWatcherHandle) {
        let (tx, rx) = mpsc::channel(32);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let handle = TaskWatcherHandle { shutdown: shutdown_tx };

        tokio::spawn(async move {
            self.run(store, tx, shutdown_rx).await;
        });

        (rx, handle)
    }

    /// Run the watcher loop.
    async fn run(
        self,
        store: Arc<SessionStore>,
        tx: mpsc::Sender<TaskCompletionEvent>,
        mut shutdown: mpsc::Receiver<()>,
    ) {
        let mut poll_timer = interval(self.poll_interval);
        let mut last_seen_hash: Option<String> = None;

        loop {
            tokio::select! {
                _ = poll_timer.tick() => {
                    if let Err(e) = self.check_completions(&store, &tx, &mut last_seen_hash).await {
                        tracing::warn!(error = %e, "failed to check task completions");
                    }
                }
                _ = shutdown.recv() => {
                    tracing::debug!("task watcher shutting down");
                    break;
                }
            }
        }
    }

    /// Check for completed tasks in egregore feed.
    async fn check_completions(
        &self,
        store: &SessionStore,
        tx: &mpsc::Sender<TaskCompletionEvent>,
        last_seen: &mut Option<String>,
    ) -> Result<()> {
        // Get all pending tasks
        let pending = store.list_all_pending_tasks()?;
        if pending.is_empty() {
            return Ok(());
        }

        // Query egregore for recent task_result messages
        let messages = self
            .egregore
            .query_messages(Some("task_result"), 50)
            .await?;

        for msg in &messages {
            // Skip if we've already seen this message
            if let Some(ref last) = last_seen {
                if &msg.hash == last {
                    break;
                }
            }

            // Check if this result relates to any pending task
            if let Some(relates) = &msg.relates {
                if let Some(task) = pending.iter().find(|t| &t.message_hash == relates) {
                    // Found a matching completion
                    let result_info = self.extract_result_info(msg);

                    // Remove from pending
                    store.remove_pending_task(&task.message_hash)?;

                    // Send completion event
                    let event = TaskCompletionEvent {
                        task: task.clone(),
                        result: result_info,
                    };

                    if tx.send(event).await.is_err() {
                        tracing::warn!("task completion receiver dropped");
                        return Ok(());
                    }
                }
            }
        }

        // Update last seen
        if let Some(first) = messages.first() {
            *last_seen = Some(first.hash.clone());
        }

        Ok(())
    }

    /// Extract result information from an egregore message.
    fn extract_result_info(&self, msg: &EgregoreMessage) -> TaskResultInfo {
        // Try to parse as task_result structure
        let (success, summary, content) = match &msg.content {
            Some(content_val) => {
                if let Ok(parsed) = serde_json::from_value::<TaskResultContent>(content_val.clone())
                {
                    (
                        parsed.success.unwrap_or(true),
                        parsed
                            .summary
                            .unwrap_or_else(|| "Task completed".to_string()),
                        parsed.result,
                    )
                } else {
                    // Fallback: treat content as summary
                    (
                        true,
                        content_val
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Task completed".to_string()),
                        None,
                    )
                }
            }
            None => (true, "Task completed".to_string(), None),
        };

        TaskResultInfo {
            hash: msg.hash.clone(),
            success,
            summary,
            content,
        }
    }
}

/// Handle to control the task watcher.
pub struct TaskWatcherHandle {
    shutdown: mpsc::Sender<()>,
}

impl TaskWatcherHandle {
    /// Stop the task watcher.
    pub async fn stop(self) {
        let _ = self.shutdown.send(()).await;
    }
}

/// Expected structure of task_result content.
#[derive(Debug, serde::Deserialize)]
struct TaskResultContent {
    success: Option<bool>,
    summary: Option<String>,
    result: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_info() {
        let info = TaskResultInfo {
            hash: "abc123".to_string(),
            success: true,
            summary: "Done".to_string(),
            content: Some("Full result here".to_string()),
        };

        assert!(info.success);
        assert_eq!(info.summary, "Done");
    }
}
