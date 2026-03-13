//! SQLite memory store implementation.

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::egregore::messages::{Task, TaskResult};
use crate::error::{Result, ServitorError};
use crate::memory::store::{ConversationTurn, MemoryStore, StoredResult, StoredTask};

/// SQLite-backed memory store.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open or create a SQLite database at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| ServitorError::Memory {
            reason: format!("failed to open database: {}", e),
        })?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize_schema()?;

        Ok(store)
    }

    /// Create an in-memory database (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| ServitorError::Memory {
            reason: format!("failed to create in-memory database: {}", e),
        })?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize_schema()?;

        Ok(store)
    }

    /// Initialize the database schema.
    fn initialize_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                hash TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                parent_id TEXT,
                priority INTEGER DEFAULT 0,
                context TEXT DEFAULT '{}',
                author TEXT,
                created_at TEXT NOT NULL,
                status TEXT DEFAULT 'pending'
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_status_priority
                ON tasks(status, priority DESC, created_at ASC);

            CREATE TABLE IF NOT EXISTS results (
                result_hash TEXT PRIMARY KEY,
                task_hash TEXT NOT NULL,
                status TEXT NOT NULL,
                result TEXT,
                error TEXT,
                attestation TEXT NOT NULL,
                created_at TEXT NOT NULL,
                synced_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_results_synced
                ON results(synced_at) WHERE synced_at IS NULL;

            CREATE TABLE IF NOT EXISTS conversation_cache (
                hash TEXT PRIMARY KEY,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                parent_hash TEXT,
                cached_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_conversation_parent
                ON conversation_cache(parent_hash);

            CREATE INDEX IF NOT EXISTS idx_conversation_expires
                ON conversation_cache(expires_at) WHERE expires_at IS NOT NULL;
            "#,
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to initialize schema: {}", e),
        })?;

        Ok(())
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn queue_task(&self, task: &Task) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let context_json = serde_json::to_string(&task.context).unwrap_or_default();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            r#"
            INSERT OR REPLACE INTO tasks
                (hash, prompt, parent_id, priority, context, author, created_at, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')
            "#,
            params![
                task.hash,
                task.prompt,
                task.parent_id,
                task.priority,
                context_json,
                task.author,
                now,
            ],
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to queue task: {}", e),
        })?;

        Ok(())
    }

    async fn next_task(&self) -> Result<Option<StoredTask>> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT hash, prompt, parent_id, priority, context, author, created_at, status
                FROM tasks
                WHERE status = 'pending'
                ORDER BY priority DESC, created_at ASC
                LIMIT 1
                "#,
            )
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to prepare query: {}", e),
            })?;

        let result = stmt
            .query_row([], |row| {
                let hash: String = row.get(0)?;
                let prompt: String = row.get(1)?;
                let parent_id: Option<String> = row.get(2)?;
                let priority: i32 = row.get(3)?;
                let context_str: String = row.get(4)?;
                let author: Option<String> = row.get(5)?;
                let created_at_str: String = row.get(6)?;
                let status: String = row.get(7)?;

                let context = serde_json::from_str(&context_str).unwrap_or_default();
                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(StoredTask {
                    hash,
                    prompt,
                    parent_id,
                    priority,
                    context,
                    author,
                    created_at,
                    status,
                })
            })
            .ok();

        Ok(result)
    }

    async fn claim_task(&self, hash: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE hash = ?1",
            params![hash],
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to claim task: {}", e),
        })?;

        Ok(())
    }

    async fn store_result(&self, result: &TaskResult) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let status_str = format!("{:?}", result.status).to_lowercase();
        let result_json = result.result.as_ref().map(|v| v.to_string());
        let attestation_json = serde_json::to_string(&result.attestation).unwrap_or_default();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            r#"
            INSERT OR REPLACE INTO results
                (result_hash, task_hash, status, result, error, attestation, created_at, synced_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
            "#,
            params![
                result.result_hash,
                result.task_hash,
                status_str,
                result_json,
                result.error,
                attestation_json,
                now,
            ],
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to store result: {}", e),
        })?;

        // Mark the task as completed
        conn.execute(
            "UPDATE tasks SET status = 'completed' WHERE hash = ?1",
            params![result.task_hash],
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to update task status: {}", e),
        })?;

        Ok(())
    }

    async fn pending_results(&self) -> Result<Vec<StoredResult>> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT result_hash, task_hash, status, result, error, attestation, created_at
                FROM results
                WHERE synced_at IS NULL
                ORDER BY created_at ASC
                "#,
            )
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to prepare query: {}", e),
            })?;

        let results = stmt
            .query_map([], |row| {
                let result_hash: String = row.get(0)?;
                let task_hash: String = row.get(1)?;
                let status: String = row.get(2)?;
                let result_str: Option<String> = row.get(3)?;
                let error: Option<String> = row.get(4)?;
                let attestation_str: String = row.get(5)?;
                let created_at_str: String = row.get(6)?;

                let result = result_str
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok());
                let attestation = serde_json::from_str(&attestation_str).unwrap_or_default();
                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(StoredResult {
                    result_hash,
                    task_hash,
                    status,
                    result,
                    error,
                    attestation,
                    created_at,
                    synced_at: None,
                })
            })
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to query results: {}", e),
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to collect results: {}", e),
            })?;

        Ok(results)
    }

    async fn mark_synced(&self, result_hash: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE results SET synced_at = ?1 WHERE result_hash = ?2",
            params![now, result_hash],
        )
        .map_err(|e| ServitorError::Memory {
            reason: format!("failed to mark synced: {}", e),
        })?;

        Ok(())
    }

    async fn fetch_history(&self, parent_id: &str) -> Result<Vec<ConversationTurn>> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT hash, role, content, parent_hash, cached_at, expires_at
                FROM conversation_cache
                WHERE parent_hash = ?1 OR hash = ?1
                ORDER BY cached_at ASC
                "#,
            )
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to prepare query: {}", e),
            })?;

        let results = stmt
            .query_map(params![parent_id], |row| {
                let hash: String = row.get(0)?;
                let role: String = row.get(1)?;
                let content: String = row.get(2)?;
                let parent_hash: Option<String> = row.get(3)?;
                let cached_at_str: String = row.get(4)?;
                let expires_at_str: Option<String> = row.get(5)?;

                let cached_at = DateTime::parse_from_rfc3339(&cached_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let expires_at = expires_at_str.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .map(|dt| dt.with_timezone(&Utc))
                        .ok()
                });

                Ok(ConversationTurn {
                    hash,
                    role,
                    content,
                    parent_hash,
                    cached_at,
                    expires_at,
                })
            })
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to query history: {}", e),
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to collect history: {}", e),
            })?;

        Ok(results)
    }

    async fn cache_history(&self, turns: &[ConversationTurn]) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        for turn in turns {
            let expires_at_str = turn.expires_at.map(|dt| dt.to_rfc3339());

            conn.execute(
                r#"
                INSERT OR REPLACE INTO conversation_cache
                    (hash, role, content, parent_hash, cached_at, expires_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    turn.hash,
                    turn.role,
                    turn.content,
                    turn.parent_hash,
                    turn.cached_at.to_rfc3339(),
                    expires_at_str,
                ],
            )
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to cache turn: {}", e),
            })?;
        }

        Ok(())
    }

    async fn clear_expired_cache(&self) -> Result<u64> {
        let conn = self.conn.lock().map_err(|_| ServitorError::Memory {
            reason: "lock poisoned".to_string(),
        })?;

        let now = Utc::now().to_rfc3339();

        let deleted = conn
            .execute(
                "DELETE FROM conversation_cache WHERE expires_at IS NOT NULL AND expires_at < ?1",
                params![now],
            )
            .map_err(|e| ServitorError::Memory {
                reason: format!("failed to clear cache: {}", e),
            })?;

        Ok(deleted as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egregore::messages::{Attestation, TaskStatus};
    use crate::identity::PublicId;
    use std::collections::HashMap;

    fn create_test_task(hash: &str, priority: i32) -> Task {
        Task {
            msg_type: "task".to_string(),
            hash: hash.to_string(),
            prompt: format!("Test task {}", hash),
            required_caps: vec![],
            parent_id: None,
            context: HashMap::new(),
            priority,
            timeout_secs: None,
            author: Some("test_author".to_string()),
            keeper: None,
        }
    }

    fn create_test_result(task_hash: &str) -> TaskResult {
        TaskResult {
            msg_type: "task_result".to_string(),
            correlation_id: "corr123".to_string(),
            task_hash: task_hash.to_string(),
            result_hash: format!("result_{}", task_hash),
            status: TaskStatus::Success,
            result: Some(serde_json::json!({"text": "done"})),
            error: None,
            attestation: Attestation {
                servitor_id: PublicId("test_servitor".to_string()),
                signature: "sig123".to_string(),
                timestamp: Utc::now(),
            },
        }
    }

    #[tokio::test]
    async fn test_queue_and_next_task() {
        let store = SqliteStore::in_memory().unwrap();

        // Queue tasks with different priorities
        let task1 = create_test_task("task1", 1);
        let task2 = create_test_task("task2", 5); // Higher priority
        let task3 = create_test_task("task3", 3);

        store.queue_task(&task1).await.unwrap();
        store.queue_task(&task2).await.unwrap();
        store.queue_task(&task3).await.unwrap();

        // Should get highest priority first
        let next = store.next_task().await.unwrap().unwrap();
        assert_eq!(next.hash, "task2");
        assert_eq!(next.priority, 5);
    }

    #[tokio::test]
    async fn test_claim_task() {
        let store = SqliteStore::in_memory().unwrap();

        let task = create_test_task("task1", 1);
        store.queue_task(&task).await.unwrap();

        // Claim the task
        store.claim_task("task1").await.unwrap();

        // Should not appear in next_task anymore
        let next = store.next_task().await.unwrap();
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn test_store_and_sync_results() {
        let store = SqliteStore::in_memory().unwrap();

        let task = create_test_task("task1", 1);
        store.queue_task(&task).await.unwrap();

        let result = create_test_result("task1");
        store.store_result(&result).await.unwrap();

        // Should be in pending
        let pending = store.pending_results().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].result_hash, "result_task1");

        // Mark as synced
        store.mark_synced("result_task1").await.unwrap();

        // Should no longer be pending
        let pending = store.pending_results().await.unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_conversation_cache() {
        let store = SqliteStore::in_memory().unwrap();

        let turns = vec![
            ConversationTurn {
                hash: "turn1".to_string(),
                role: "user".to_string(),
                content: "Hello".to_string(),
                parent_hash: None,
                cached_at: Utc::now(),
                expires_at: None,
            },
            ConversationTurn {
                hash: "turn2".to_string(),
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
                parent_hash: Some("turn1".to_string()),
                cached_at: Utc::now(),
                expires_at: None,
            },
        ];

        store.cache_history(&turns).await.unwrap();

        // Fetch by parent hash
        let history = store.fetch_history("turn1").await.unwrap();
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn test_clear_expired_cache() {
        let store = SqliteStore::in_memory().unwrap();

        let expired_turn = ConversationTurn {
            hash: "expired".to_string(),
            role: "user".to_string(),
            content: "Old message".to_string(),
            parent_hash: None,
            cached_at: Utc::now(),
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        };

        store.cache_history(&[expired_turn]).await.unwrap();

        let deleted = store.clear_expired_cache().await.unwrap();
        assert_eq!(deleted, 1);
    }
}
