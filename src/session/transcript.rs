//! JSONL transcript handling for sessions.
//!
//! Each session has an append-only JSONL file containing all messages
//! exchanged during the session.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::SessionId;
use crate::error::{Result, ServitorError};

/// Role of the message sender.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Message from the keeper (user).
    User,
    /// Message from the servitor (assistant).
    Assistant,
    /// System message (context, errors).
    System,
    /// Tool invocation.
    Tool,
    /// Tool result.
    ToolResult,
}

/// A single entry in the session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// When this entry was recorded.
    pub timestamp: DateTime<Utc>,

    /// Role of the sender.
    pub role: Role,

    /// Message content.
    pub content: String,

    /// Optional metadata (tool name, task hash, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl TranscriptEntry {
    /// Create a user message entry.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            role: Role::User,
            content: content.into(),
            metadata: None,
        }
    }

    /// Create an assistant message entry.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            role: Role::Assistant,
            content: content.into(),
            metadata: None,
        }
    }

    /// Create a system message entry.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            role: Role::System,
            content: content.into(),
            metadata: None,
        }
    }

    /// Create a tool invocation entry.
    pub fn tool(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            timestamp: Utc::now(),
            role: Role::Tool,
            content: name.into(),
            metadata: Some(args),
        }
    }

    /// Create a tool result entry.
    pub fn tool_result(name: impl Into<String>, result: serde_json::Value) -> Self {
        Self {
            timestamp: Utc::now(),
            role: Role::ToolResult,
            content: name.into(),
            metadata: Some(result),
        }
    }

    /// Add metadata to this entry.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Writer for session transcripts.
pub struct TranscriptWriter {
    sessions_dir: PathBuf,
}

impl TranscriptWriter {
    /// Create a new transcript writer.
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: sessions_dir.into(),
        }
    }

    /// Get the path to a session's transcript file.
    pub fn transcript_path(&self, session_id: &SessionId) -> PathBuf {
        self.sessions_dir.join(format!("{}.jsonl", session_id))
    }

    /// Append an entry to a session's transcript.
    pub fn append(&self, session_id: &SessionId, entry: &TranscriptEntry) -> Result<()> {
        // Ensure sessions directory exists
        std::fs::create_dir_all(&self.sessions_dir).map_err(|e| ServitorError::Session {
            reason: format!("failed to create sessions directory: {}", e),
        })?;

        let path = self.transcript_path(session_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ServitorError::Session {
                reason: format!("failed to open transcript {}: {}", path.display(), e),
            })?;

        let line = serde_json::to_string(entry).map_err(|e| ServitorError::Session {
            reason: format!("failed to serialize transcript entry: {}", e),
        })?;

        writeln!(file, "{}", line).map_err(|e| ServitorError::Session {
            reason: format!("failed to write transcript entry: {}", e),
        })?;

        Ok(())
    }

    /// Read all entries from a session's transcript.
    pub fn read(&self, session_id: &SessionId) -> Result<Vec<TranscriptEntry>> {
        let path = self.transcript_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&path).map_err(|e| ServitorError::Session {
            reason: format!("failed to open transcript {}: {}", path.display(), e),
        })?;

        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| ServitorError::Session {
                reason: format!("failed to read transcript line {}: {}", line_num + 1, e),
            })?;

            if line.trim().is_empty() {
                continue;
            }

            let entry: TranscriptEntry =
                serde_json::from_str(&line).map_err(|e| ServitorError::Session {
                    reason: format!("failed to parse transcript line {}: {}", line_num + 1, e),
                })?;

            entries.push(entry);
        }

        Ok(entries)
    }

    /// Read the last N entries from a session's transcript.
    pub fn read_recent(
        &self,
        session_id: &SessionId,
        count: usize,
    ) -> Result<Vec<TranscriptEntry>> {
        let entries = self.read(session_id)?;
        let start = entries.len().saturating_sub(count);
        Ok(entries[start..].to_vec())
    }

    /// Check if a transcript exists.
    pub fn exists(&self, session_id: &SessionId) -> bool {
        self.transcript_path(session_id).exists()
    }

    /// Delete a transcript file.
    pub fn delete(&self, session_id: &SessionId) -> Result<()> {
        let path = self.transcript_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| ServitorError::Session {
                reason: format!("failed to delete transcript {}: {}", path.display(), e),
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcript_entry_creation() {
        let user = TranscriptEntry::user("Hello");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.content, "Hello");

        let assistant = TranscriptEntry::assistant("Hi there");
        assert_eq!(assistant.role, Role::Assistant);

        let tool = TranscriptEntry::tool("search", serde_json::json!({"query": "test"}));
        assert_eq!(tool.role, Role::Tool);
        assert!(tool.metadata.is_some());
    }

    #[test]
    fn test_transcript_serialization() {
        let entry = TranscriptEntry::user("Test message");
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"Test message\""));

        let parsed: TranscriptEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, entry.role);
        assert_eq!(parsed.content, entry.content);
    }

    #[test]
    fn test_transcript_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let writer = TranscriptWriter::new(temp_dir.path().join("sessions"));
        let session_id = "test-session".to_string();

        // Write entries
        writer
            .append(&session_id, &TranscriptEntry::user("Hello"))
            .unwrap();
        writer
            .append(&session_id, &TranscriptEntry::assistant("Hi there"))
            .unwrap();
        writer
            .append(&session_id, &TranscriptEntry::user("How are you?"))
            .unwrap();

        // Read back
        let entries = writer.read(&session_id).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].content, "Hello");
        assert_eq!(entries[1].content, "Hi there");
        assert_eq!(entries[2].content, "How are you?");

        // Read recent
        let recent = writer.read_recent(&session_id, 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "Hi there");
        assert_eq!(recent[1].content, "How are you?");
    }
}
