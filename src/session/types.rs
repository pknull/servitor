//! Session types and data structures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique session identifier.
pub type SessionId = String;

/// Transport through which a session was initiated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transport {
    /// A2A protocol (agent-to-agent).
    A2a {
        /// Callback URL for task completion notification.
        callback_url: Option<String>,
        /// Originating agent's public ID.
        agent_id: String,
    },

    /// Command-line interface.
    Cli,

    /// Egregore feed (hook mode).
    Egregore {
        /// Author public key who sent the task.
        author: String,
    },
}

impl Transport {
    /// Create an A2A transport.
    pub fn a2a(agent_id: impl Into<String>, callback_url: Option<String>) -> Self {
        Self::A2a {
            agent_id: agent_id.into(),
            callback_url,
        }
    }

    /// Create an Egregore transport.
    pub fn egregore(author: impl Into<String>) -> Self {
        Self::Egregore {
            author: author.into(),
        }
    }

    /// Get a stable key for session lookup.
    ///
    /// Sessions are keyed by (keeper, channel context).
    pub fn session_key(&self, keeper: &str) -> String {
        match self {
            Transport::A2a { agent_id, .. } => {
                format!("a2a:{}:{}", keeper, agent_id)
            }
            Transport::Cli => {
                format!("cli:{}", keeper)
            }
            Transport::Egregore { author } => {
                format!("egregore:{}:{}", keeper, author)
            }
        }
    }

    /// Get the transport type as a string.
    pub fn transport_type(&self) -> &'static str {
        match self {
            Transport::A2a { .. } => "a2a",
            Transport::Cli => "cli",
            Transport::Egregore { .. } => "egregore",
        }
    }
}

/// A session representing an ongoing interaction with a keeper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session identifier.
    pub id: SessionId,

    /// Name of the keeper this session belongs to.
    pub keeper: String,

    /// Transport through which the session was initiated.
    pub transport: Transport,

    /// When the session was created.
    pub created_at: DateTime<Utc>,

    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,

    /// Session state.
    pub state: SessionState,
}

/// Session state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Session is active and can receive messages.
    #[default]
    Active,

    /// Session is waiting for delegated task completion.
    AwaitingTask,

    /// Session has been closed.
    Closed,
}

impl Session {
    /// Create a new session.
    pub fn new(keeper: impl Into<String>, transport: Transport) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            keeper: keeper.into(),
            transport,
            created_at: now,
            updated_at: now,
            state: SessionState::Active,
        }
    }

    /// Mark session as updated.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// A pending task that the session is waiting for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTask {
    /// Session this task belongs to.
    pub session_id: SessionId,

    /// Hash of the message we sent (for `relates` matching).
    pub message_hash: String,

    /// What we're waiting for (human-readable).
    pub description: String,

    /// When the task was delegated.
    pub created_at: DateTime<Utc>,

    /// Target agent or service that's handling the task.
    pub target: String,
}

impl PendingTask {
    /// Create a new pending task.
    pub fn new(
        session_id: SessionId,
        message_hash: impl Into<String>,
        description: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            message_hash: message_hash.into(),
            description: description.into(),
            created_at: Utc::now(),
            target: target.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_session_key() {
        let a2a = Transport::a2a("agent1", Some("http://callback".into()));
        assert_eq!(a2a.session_key("bob"), "a2a:bob:agent1");

        let cli = Transport::Cli;
        assert_eq!(cli.session_key("charlie"), "cli:charlie");

        let egregore = Transport::egregore("@pubkey.ed25519");
        assert_eq!(
            egregore.session_key("dave"),
            "egregore:dave:@pubkey.ed25519"
        );
    }

    #[test]
    fn test_session_creation() {
        let session = Session::new("alice", Transport::Cli);
        assert_eq!(session.keeper, "alice");
        assert_eq!(session.state, SessionState::Active);
        assert!(!session.id.is_empty());
    }

    #[test]
    fn test_transport_serialization() {
        let a2a = Transport::a2a("agent1", Some("http://callback".into()));
        let json = serde_json::to_string(&a2a).unwrap();
        assert!(json.contains("\"type\":\"a2a\""));

        let parsed: Transport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, a2a);
    }
}
