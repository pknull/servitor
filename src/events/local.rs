//! Local event source — reads tasks from SQLite queue.

use std::sync::Arc;

use async_trait::async_trait;

use crate::egregore::Task;
use crate::events::EventSource;
use crate::memory::MemoryStore;

/// Event source that reads tasks from local memory queue.
pub struct LocalSource {
    store: Arc<dyn MemoryStore>,
}

impl LocalSource {
    /// Create a new local source from a memory store.
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl EventSource for LocalSource {
    async fn next(&mut self) -> Option<Task> {
        match self.store.next_task().await {
            Ok(Some(stored)) => {
                // Claim the task to prevent double-processing
                if let Err(e) = self.store.claim_task(&stored.hash).await {
                    tracing::warn!(hash = %stored.hash, error = %e, "failed to claim task");
                    return None;
                }

                tracing::debug!(
                    hash = %stored.hash,
                    priority = stored.priority,
                    "dequeued task from local storage"
                );

                Some(stored.to_task())
            }
            Ok(None) => None,
            Err(e) => {
                tracing::error!(error = %e, "failed to read from local queue");
                None
            }
        }
    }

    fn name(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SqliteStore;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_local_source() {
        let store = Arc::new(SqliteStore::in_memory().unwrap());

        // Queue a task
        let task = crate::egregore::messages::Task {
            msg_type: "task".to_string(),
            hash: "local_task".to_string(),
            prompt: "Local task".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: HashMap::new(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };

        store.queue_task(&task).await.unwrap();

        let mut source = LocalSource::new(store.clone());

        // Should get the task
        let next = source.next().await.unwrap();
        assert_eq!(next.hash, "local_task");

        // Should not get it again (claimed)
        let next = source.next().await;
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let store = Arc::new(SqliteStore::in_memory().unwrap());

        // Queue tasks with different priorities
        for (hash, priority) in [("low", 1), ("high", 10), ("med", 5)] {
            let task = crate::egregore::messages::Task {
                msg_type: "task".to_string(),
                hash: hash.to_string(),
                prompt: format!("{} priority task", hash),
                required_caps: vec![],
                parent_id: None,
                context: HashMap::new(),
                priority,
                timeout_secs: None,
                author: None,
                keeper: None,
            };
            store.queue_task(&task).await.unwrap();
        }

        let mut source = LocalSource::new(store);

        // Should get highest priority first
        assert_eq!(source.next().await.unwrap().hash, "high");
        assert_eq!(source.next().await.unwrap().hash, "med");
        assert_eq!(source.next().await.unwrap().hash, "low");
        assert!(source.next().await.is_none());
    }
}
