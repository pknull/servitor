//! Communication transport layer.
//!
//! Provides pluggable transports for receiving messages and sending responses.
//!
//! The current runtime wires Discord transport only. Some additional transport
//! schemas remain in config as reserved future surfaces.

pub mod discord;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Message received from a communication transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommsMessage {
    /// Which transport this came from.
    pub source: CommsSource,

    /// Channel identifier (transport-specific format).
    pub channel_id: String,

    /// User identifier (transport-specific format).
    pub user_id: String,

    /// Display name of the user.
    pub user_name: String,

    /// Message content.
    pub content: String,

    /// Thread parent message ID (if in a thread).
    pub reply_to: Option<String>,

    /// Original message ID (for threading responses).
    pub message_id: String,

    /// When the message was received.
    pub timestamp: DateTime<Utc>,
}

/// Source transport identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommsSource {
    Discord {
        guild_id: String,
        guild_name: String,
    },
    Http {
        endpoint: String,
    },
    Matrix {
        room_id: String,
    },
}

impl CommsSource {
    pub fn name(&self) -> &'static str {
        match self {
            CommsSource::Discord { .. } => "discord",
            CommsSource::Http { .. } => "http",
            CommsSource::Matrix { .. } => "matrix",
        }
    }
}

/// Response to send back through a transport.
#[derive(Debug, Clone)]
pub struct CommsResponse {
    /// Channel to send to.
    pub channel_id: String,

    /// Message to reply to (for threading).
    pub reply_to: Option<String>,

    /// Response content.
    pub content: String,
}

/// Handle for sending responses back through a transport.
#[async_trait]
pub trait CommsResponder: Send + Sync {
    /// Send a response.
    async fn send(&self, response: CommsResponse) -> Result<()>;
}

/// Communication transport trait.
#[async_trait]
pub trait CommsTransport: Send {
    /// Transport name.
    fn name(&self) -> &str;

    /// Connect to the transport.
    async fn connect(&mut self) -> Result<()>;

    /// Receive the next message (blocks until available).
    async fn recv(&mut self) -> Option<(CommsMessage, Box<dyn CommsResponder>)>;

    /// Disconnect from the transport.
    async fn disconnect(&mut self) -> Result<()>;
}
