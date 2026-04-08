//! SQLite-backed session store.
//!
//! Provides indexed access to sessions and pending task correlations.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::transcript::TranscriptWriter;
use super::types::{PendingTask, Session, SessionId, SessionState, Transport};
use crate::error::{Result, ServitorError};

/// SQLite-backed session store.
///
/// Thread-safe via internal Mutex on the connection.
pub struct SessionStore {
    conn: Mutex<Connection>,
    transcript_writer: TranscriptWriter,
    #[allow(dead_code)]
    sessions_dir: PathBuf,
}

// Safety: SessionStore is Send because Mutex<Connection> is Send
unsafe impl Send for SessionStore {}
unsafe impl Sync for SessionStore {}

impl SessionStore {
    /// Open or create a session store.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let sessions_dir = data_dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir).map_err(|e| ServitorError::Session {
            reason: format!("failed to create sessions directory: {}", e),
        })?;

        let db_path = sessions_dir.join("index.sqlite");
        let conn = Connection::open(&db_path)?;

        let store = Self {
            conn: Mutex::new(conn),
            transcript_writer: TranscriptWriter::new(&sessions_dir),
            sessions_dir,
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Open an in-memory session store (for testing).
    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = Connection::open_in_memory()?;

        let store = Self {
            conn: Mutex::new(conn),
            transcript_writer: TranscriptWriter::new(&sessions_dir),
            sessions_dir,
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Initialize database schema.
    fn initialize_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        conn.execute_batch(
            r#"
            -- Sessions table
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                keeper TEXT NOT NULL,
                transport_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            -- Index for session lookup by keeper
            CREATE INDEX IF NOT EXISTS idx_sessions_keeper ON sessions(keeper);

            -- Index for session lookup by transport key
            CREATE INDEX IF NOT EXISTS idx_sessions_transport ON sessions(transport_json);

            -- Pending tasks table (correlations awaiting response)
            CREATE TABLE IF NOT EXISTS pending_tasks (
                message_hash TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                description TEXT NOT NULL,
                target TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            -- Index for pending tasks by session
            CREATE INDEX IF NOT EXISTS idx_pending_session ON pending_tasks(session_id);

            -- Session keys table (for fast lookup by transport key)
            CREATE TABLE IF NOT EXISTS session_keys (
                key TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            "#,
        )?;

        Ok(())
    }

    /// Get the transcript writer.
    pub fn transcript_writer(&self) -> &TranscriptWriter {
        &self.transcript_writer
    }

    /// Create a new session.
    pub fn create_session(&self, keeper: &str, transport: &Transport) -> Result<Session> {
        let session = Session::new(keeper, transport.clone());
        let transport_json =
            serde_json::to_string(transport).map_err(|e| ServitorError::Session {
                reason: format!("failed to serialize transport: {}", e),
            })?;

        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        conn.execute(
            "INSERT INTO sessions (id, keeper, transport_json, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &session.id,
                keeper,
                transport_json,
                session_state_to_str(&session.state),
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )?;

        // Store the session key for fast lookup
        let key = transport.session_key(keeper);
        conn.execute(
            "INSERT OR REPLACE INTO session_keys (key, session_id) VALUES (?1, ?2)",
            params![key, &session.id],
        )?;

        Ok(session)
    }

    /// Get a session by ID.
    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<Session>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, keeper, transport_json, state, created_at, updated_at
             FROM sessions WHERE id = ?1",
        )?;

        stmt.query_row(params![session_id], |row| Ok(Self::row_to_session(row)))
            .optional()?
            .transpose()
    }

    /// Find or create a session for a keeper and transport.
    pub fn find_or_create_session(&self, keeper: &str, transport: &Transport) -> Result<Session> {
        let key = transport.session_key(keeper);

        // Try to find existing session by key
        let existing = self.get_session_by_key(&key)?;
        if let Some(session) = existing {
            return Ok(session);
        }

        // Create new session
        self.create_session(keeper, transport)
    }

    /// Get a session by its transport key.
    pub fn get_session_by_key(&self, key: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let session_id: Option<String> = conn
            .query_row(
                "SELECT session_id FROM session_keys WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;

        drop(conn); // Release lock before recursive call

        match session_id {
            Some(id) => self.get_session(&id),
            None => Ok(None),
        }
    }

    /// Update a session.
    pub fn update_session(&self, session: &Session) -> Result<()> {
        let transport_json =
            serde_json::to_string(&session.transport).map_err(|e| ServitorError::Session {
                reason: format!("failed to serialize transport: {}", e),
            })?;

        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        conn.execute(
            "UPDATE sessions SET keeper = ?1, transport_json = ?2, state = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                &session.keeper,
                transport_json,
                session_state_to_str(&session.state),
                session.updated_at.to_rfc3339(),
                &session.id,
            ],
        )?;

        Ok(())
    }

    /// List all sessions for a keeper.
    pub fn list_sessions_for_keeper(&self, keeper: &str) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, keeper, transport_json, state, created_at, updated_at
             FROM sessions WHERE keeper = ?1 ORDER BY updated_at DESC",
        )?;

        let sessions = stmt
            .query_map(params![keeper], |row| Ok(Self::row_to_session(row)))?
            .filter_map(|r| r.ok())
            .filter_map(|r| r.ok())
            .collect();

        Ok(sessions)
    }

    /// List all active sessions.
    pub fn list_active_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, keeper, transport_json, state, created_at, updated_at
             FROM sessions WHERE state = 'active' OR state = 'awaiting_task'
             ORDER BY updated_at DESC",
        )?;

        let sessions = stmt
            .query_map([], |row| Ok(Self::row_to_session(row)))?
            .filter_map(|r| r.ok())
            .filter_map(|r| r.ok())
            .collect();

        Ok(sessions)
    }

    /// Delete a session and its transcript.
    pub fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        // Delete from database (cascades to pending_tasks and session_keys)
        conn.execute(
            "DELETE FROM session_keys WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM pending_tasks WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;

        drop(conn); // Release lock before file operation

        // Delete transcript file
        self.transcript_writer.delete(session_id)?;

        Ok(())
    }

    /// Add a pending task correlation.
    pub fn add_pending_task(&self, task: &PendingTask) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        conn.execute(
            "INSERT INTO pending_tasks (message_hash, session_id, description, target, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &task.message_hash,
                &task.session_id,
                &task.description,
                &task.target,
                task.created_at.to_rfc3339(),
            ],
        )?;

        // Update session state to awaiting_task
        conn.execute(
            "UPDATE sessions SET state = 'awaiting_task', updated_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), &task.session_id],
        )?;

        Ok(())
    }

    /// Get a pending task by message hash.
    pub fn get_pending_task(&self, message_hash: &str) -> Result<Option<PendingTask>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT message_hash, session_id, description, target, created_at
             FROM pending_tasks WHERE message_hash = ?1",
        )?;

        stmt.query_row(params![message_hash], |row| {
            let created_at: String = row.get(4)?;
            Ok(PendingTask {
                message_hash: row.get(0)?,
                session_id: row.get(1)?,
                description: row.get(2)?,
                target: row.get(3)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .unwrap_or_else(|_| Utc::now().into())
                    .with_timezone(&Utc),
            })
        })
        .optional()
        .map_err(Into::into)
    }

    /// Remove a pending task (task completed).
    pub fn remove_pending_task(&self, message_hash: &str) -> Result<Option<PendingTask>> {
        let task = self.get_pending_task(message_hash)?;

        if task.is_some() {
            let conn = self.conn.lock().map_err(|e| ServitorError::Session {
                reason: format!("failed to acquire lock: {}", e),
            })?;

            conn.execute(
                "DELETE FROM pending_tasks WHERE message_hash = ?1",
                params![message_hash],
            )?;
        }

        Ok(task)
    }

    /// List all pending tasks for a session.
    pub fn list_pending_tasks(&self, session_id: &SessionId) -> Result<Vec<PendingTask>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT message_hash, session_id, description, target, created_at
             FROM pending_tasks WHERE session_id = ?1 ORDER BY created_at",
        )?;

        let tasks = stmt
            .query_map(params![session_id], |row| {
                let created_at: String = row.get(4)?;
                Ok(PendingTask {
                    message_hash: row.get(0)?,
                    session_id: row.get(1)?,
                    description: row.get(2)?,
                    target: row.get(3)?,
                    created_at: DateTime::parse_from_rfc3339(&created_at)
                        .unwrap_or_else(|_| Utc::now().into())
                        .with_timezone(&Utc),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    }

    /// List all pending tasks across all sessions.
    pub fn list_all_pending_tasks(&self) -> Result<Vec<PendingTask>> {
        let conn = self.conn.lock().map_err(|e| ServitorError::Session {
            reason: format!("failed to acquire lock: {}", e),
        })?;

        let mut stmt = conn.prepare(
            "SELECT message_hash, session_id, description, target, created_at
             FROM pending_tasks ORDER BY created_at",
        )?;

        let tasks = stmt
            .query_map([], |row| {
                let created_at: String = row.get(4)?;
                Ok(PendingTask {
                    message_hash: row.get(0)?,
                    session_id: row.get(1)?,
                    description: row.get(2)?,
                    target: row.get(3)?,
                    created_at: DateTime::parse_from_rfc3339(&created_at)
                        .unwrap_or_else(|_| Utc::now().into())
                        .with_timezone(&Utc),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    }

    /// Convert a database row to a Session.
    fn row_to_session(row: &rusqlite::Row) -> Result<Session> {
        let id: String = row.get(0)?;
        let keeper: String = row.get(1)?;
        let transport_json: String = row.get(2)?;
        let state_str: String = row.get(3)?;
        let created_at_str: String = row.get(4)?;
        let updated_at_str: String = row.get(5)?;

        let transport: Transport =
            serde_json::from_str(&transport_json).map_err(|e| ServitorError::Session {
                reason: format!("failed to parse transport: {}", e),
            })?;

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .unwrap_or_else(|_| Utc::now().into())
            .with_timezone(&Utc);

        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .unwrap_or_else(|_| Utc::now().into())
            .with_timezone(&Utc);

        Ok(Session {
            id,
            keeper,
            transport,
            state: str_to_session_state(&state_str),
            created_at,
            updated_at,
        })
    }
}

fn session_state_to_str(state: &SessionState) -> &'static str {
    match state {
        SessionState::Active => "active",
        SessionState::AwaitingTask => "awaiting_task",
        SessionState::Closed => "closed",
    }
}

fn str_to_session_state(s: &str) -> SessionState {
    match s {
        "active" => SessionState::Active,
        "awaiting_task" => SessionState::AwaitingTask,
        "closed" => SessionState::Closed,
        _ => SessionState::Active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_store_create_and_get() {
        let store = SessionStore::open_memory().unwrap();
        let transport = Transport::Cli;

        let session = store.create_session("alice", &transport).unwrap();
        assert_eq!(session.keeper, "alice");
        assert_eq!(session.state, SessionState::Active);

        let retrieved = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(retrieved.id, session.id);
        assert_eq!(retrieved.keeper, "alice");
    }

    #[test]
    fn test_session_store_find_or_create() {
        let store = SessionStore::open_memory().unwrap();
        let transport = Transport::a2a("agent1", None);

        // First call creates
        let session1 = store.find_or_create_session("alice", &transport).unwrap();

        // Second call finds existing
        let session2 = store.find_or_create_session("alice", &transport).unwrap();
        assert_eq!(session1.id, session2.id);

        // Different transport creates new session
        let transport2 = Transport::a2a("agent2", None);
        let session3 = store.find_or_create_session("alice", &transport2).unwrap();
        assert_ne!(session1.id, session3.id);
    }

    #[test]
    fn test_pending_tasks() {
        let store = SessionStore::open_memory().unwrap();
        let transport = Transport::Cli;
        let session = store.create_session("bob", &transport).unwrap();

        // Add pending task
        let task = PendingTask::new(session.id.clone(), "hash123", "Test task", "external-agent");
        store.add_pending_task(&task).unwrap();

        // Session should be awaiting_task
        let updated = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(updated.state, SessionState::AwaitingTask);

        // Find task by hash
        let found = store.get_pending_task("hash123").unwrap().unwrap();
        assert_eq!(found.session_id, session.id);
        assert_eq!(found.description, "Test task");

        // Remove task
        let removed = store.remove_pending_task("hash123").unwrap().unwrap();
        assert_eq!(removed.message_hash, "hash123");

        // Should be gone
        assert!(store.get_pending_task("hash123").unwrap().is_none());
    }

    #[test]
    fn test_list_sessions() {
        let store = SessionStore::open_memory().unwrap();

        store.create_session("alice", &Transport::Cli).unwrap();
        store
            .create_session("alice", &Transport::a2a("agent1", None))
            .unwrap();
        store.create_session("bob", &Transport::Cli).unwrap();

        let alice_sessions = store.list_sessions_for_keeper("alice").unwrap();
        assert_eq!(alice_sessions.len(), 2);

        let bob_sessions = store.list_sessions_for_keeper("bob").unwrap();
        assert_eq!(bob_sessions.len(), 1);

        let all_active = store.list_active_sessions().unwrap();
        assert_eq!(all_active.len(), 3);
    }
}
