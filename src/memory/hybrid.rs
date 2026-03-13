//! Hybrid memory store — local SQLite with egregore sync.
//!
//! Combines local storage with egregore fallback for conversation history.
//! In hybrid mode:
//! - Tasks are queued locally
//! - Results are stored locally, then synced to egregore
//! - Conversation history checks local cache, falls back to egregore

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};

use crate::egregore::messages::{Task, TaskResult};
use crate::egregore::EgregoreClient;
use crate::error::Result;
use crate::memory::sqlite::SqliteStore;
use crate::memory::store::{ConversationTurn, MemoryStore, StoredResult, StoredTask};

/// Hybrid memory store combining local SQLite with egregore.
pub struct HybridStore {
    local: Arc<SqliteStore>,
    egregore: Option<Arc<EgregoreClient>>,
    cache_ttl_hours: u64,
}

impl HybridStore {
    /// Create a new hybrid store.
    pub fn new(local: SqliteStore, egregore: Option<EgregoreClient>, cache_ttl_hours: u64) -> Self {
        Self {
            local: Arc::new(local),
            egregore: egregore.map(Arc::new),
            cache_ttl_hours,
        }
    }

    /// Create with just local storage (no egregore).
    pub fn local_only(local: SqliteStore, cache_ttl_hours: u64) -> Self {
        Self {
            local: Arc::new(local),
            egregore: None,
            cache_ttl_hours,
        }
    }

    /// Attach an egregore client for sync operations.
    pub fn with_egregore(mut self, egregore: EgregoreClient) -> Self {
        self.egregore = Some(Arc::new(egregore));
        self
    }

    /// Check if egregore is available.
    pub fn has_egregore(&self) -> bool {
        self.egregore.is_some()
    }

    /// Get the underlying local store for direct access.
    pub fn local(&self) -> &SqliteStore {
        &self.local
    }

    /// Sync pending results to egregore.
    /// Returns the number of results synced.
    pub async fn sync_results(&self, batch_size: usize) -> Result<usize> {
        let egregore = match &self.egregore {
            Some(e) => e,
            None => return Ok(0),
        };

        let pending = self.local.pending_results().await?;
        let mut synced = 0;

        for result in pending.into_iter().take(batch_size) {
            // Reconstruct TaskResult from StoredResult
            let task_result = self.stored_to_task_result(&result)?;

            match egregore.publish_result(&task_result).await {
                Ok(_) => {
                    self.local.mark_synced(&result.result_hash).await?;
                    synced += 1;
                    tracing::debug!(
                        result_hash = %result.result_hash,
                        "synced result to egregore"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        result_hash = %result.result_hash,
                        error = %e,
                        "failed to sync result, will retry"
                    );
                    // Don't mark as synced, will retry next time
                    break; // Stop batch on first failure
                }
            }
        }

        Ok(synced)
    }

    /// Convert StoredResult back to TaskResult for publishing.
    fn stored_to_task_result(&self, stored: &StoredResult) -> Result<TaskResult> {
        use crate::egregore::messages::{Attestation, TaskStatus};
        use crate::error::ServitorError;

        let status = match stored.status.as_str() {
            "success" => TaskStatus::Success,
            "error" => TaskStatus::Error,
            "timeout" => TaskStatus::Timeout,
            _ => TaskStatus::Error,
        };

        let attestation: Attestation =
            serde_json::from_value(stored.attestation.clone()).map_err(|e| {
                ServitorError::Memory {
                    reason: format!("invalid attestation: {}", e),
                }
            })?;

        Ok(TaskResult {
            msg_type: "task_result".to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            task_hash: stored.task_hash.clone(),
            result_hash: stored.result_hash.clone(),
            status,
            result: stored.result.clone(),
            error: stored.error.clone(),
            attestation,
        })
    }
}

#[async_trait]
impl MemoryStore for HybridStore {
    async fn queue_task(&self, task: &Task) -> Result<()> {
        self.local.queue_task(task).await
    }

    async fn next_task(&self) -> Result<Option<StoredTask>> {
        self.local.next_task().await
    }

    async fn claim_task(&self, hash: &str) -> Result<()> {
        self.local.claim_task(hash).await
    }

    async fn store_result(&self, result: &TaskResult) -> Result<()> {
        self.local.store_result(result).await
    }

    async fn pending_results(&self) -> Result<Vec<StoredResult>> {
        self.local.pending_results().await
    }

    async fn mark_synced(&self, result_hash: &str) -> Result<()> {
        self.local.mark_synced(result_hash).await
    }

    async fn fetch_history(&self, parent_id: &str) -> Result<Vec<ConversationTurn>> {
        // First check local cache
        let cached = self.local.fetch_history(parent_id).await?;
        if !cached.is_empty() {
            tracing::debug!(parent_id = %parent_id, turns = cached.len(), "using cached history");
            return Ok(cached);
        }

        // Fall back to egregore if available
        if let Some(ref egregore) = self.egregore {
            match egregore.fetch_conversation_history(parent_id).await {
                Ok(history) if !history.is_empty() => {
                    // Convert egregore ConversationTurn to memory ConversationTurn and cache
                    let expires_at =
                        Some(Utc::now() + Duration::hours(self.cache_ttl_hours as i64));
                    let turns: Vec<ConversationTurn> = history
                        .iter()
                        .map(|turn| ConversationTurn {
                            hash: turn.hash.clone(),
                            role: turn.role.clone(),
                            content: turn.content.clone(),
                            parent_hash: None, // Egregore's ConversationTurn doesn't track parent
                            cached_at: Utc::now(),
                            expires_at,
                        })
                        .collect();

                    // Cache for future use
                    if let Err(e) = self.local.cache_history(&turns).await {
                        tracing::warn!(error = %e, "failed to cache conversation history");
                    }

                    tracing::debug!(
                        parent_id = %parent_id,
                        turns = turns.len(),
                        "fetched and cached history from egregore"
                    );
                    return Ok(turns);
                }
                Ok(_) => {
                    tracing::debug!(parent_id = %parent_id, "no history found in egregore");
                }
                Err(e) => {
                    tracing::warn!(
                        parent_id = %parent_id,
                        error = %e,
                        "failed to fetch history from egregore"
                    );
                }
            }
        }

        Ok(vec![])
    }

    async fn cache_history(&self, turns: &[ConversationTurn]) -> Result<()> {
        self.local.cache_history(turns).await
    }

    async fn clear_expired_cache(&self) -> Result<u64> {
        self.local.clear_expired_cache().await
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hybrid_local_only() {
        let local = SqliteStore::in_memory().unwrap();
        let hybrid = HybridStore::local_only(local, 168);

        assert!(!hybrid.has_egregore());

        // Should work for local operations
        let task = crate::egregore::messages::Task {
            msg_type: "task".to_string(),
            hash: "test123".to_string(),
            prompt: "Test".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: std::collections::HashMap::new(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };

        hybrid.queue_task(&task).await.unwrap();
        let next = hybrid.next_task().await.unwrap().unwrap();
        assert_eq!(next.hash, "test123");
    }

    #[tokio::test]
    async fn test_sync_without_egregore() {
        let local = SqliteStore::in_memory().unwrap();
        let hybrid = HybridStore::local_only(local, 168);

        // Sync should return 0 when no egregore
        let synced = hybrid.sync_results(10).await.unwrap();
        assert_eq!(synced, 0);
    }
}
