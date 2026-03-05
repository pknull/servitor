//! Egregore message schemas for Servitor.
//!
//! These are the content types that Servitor publishes to and receives from
//! the egregore network.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::identity::PublicId;

/// Servitor capability profile, published on startup and heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServitorProfile {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Servitor's public identity.
    pub servitor_id: PublicId,

    /// Capability classes available.
    pub capabilities: Vec<String>,

    /// Tool names available (prefixed with MCP server name).
    pub tools: Vec<String>,

    /// Scope constraints per capability.
    pub scopes: HashMap<String, ScopeConstraints>,

    /// Resource limits.
    #[serde(default)]
    pub resource_limits: ResourceLimits,

    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,

    /// Profile version.
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "1.0".to_string()
}

impl ServitorProfile {
    pub fn new(servitor_id: PublicId, heartbeat_interval_ms: u64) -> Self {
        Self {
            msg_type: "servitor_profile".to_string(),
            servitor_id,
            capabilities: vec![],
            tools: vec![],
            scopes: HashMap::new(),
            resource_limits: ResourceLimits::default(),
            heartbeat_interval_ms,
            version: default_version(),
        }
    }
}

/// Scope constraints for a capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConstraints {
    pub allow: Vec<String>,
    pub block: Vec<String>,
}

/// Resource limits for this Servitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: u32,
    pub memory_mb: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu: 2,
            memory_mb: 4096,
        }
    }
}

/// Task claim message — claim a task before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskClaim {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Correlation ID for tracking.
    pub correlation_id: String,

    /// Hash of the task being claimed.
    pub task_hash: String,

    /// Servitor claiming the task.
    pub servitor_id: PublicId,

    /// Time-to-live in seconds.
    pub ttl_seconds: u64,

    /// Claim timestamp.
    pub timestamp: DateTime<Utc>,
}

impl TaskClaim {
    pub fn new(task_hash: String, servitor_id: PublicId, ttl_seconds: u64) -> Self {
        Self {
            msg_type: "task_claim".to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            task_hash,
            servitor_id,
            ttl_seconds,
            timestamp: Utc::now(),
        }
    }
}

/// Task result with signed attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Correlation ID matching the claim.
    pub correlation_id: String,

    /// Hash of the task.
    pub task_hash: String,

    /// Hash of the result content.
    pub result_hash: String,

    /// Execution status.
    pub status: TaskStatus,

    /// Result content (when successful).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error message (when failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Signed attestation.
    pub attestation: Attestation,
}

/// Task execution status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Success,
    Error,
    Timeout,
}

/// Signed attestation binding identity to output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Servitor that executed the task.
    pub servitor_id: PublicId,

    /// Ed25519 signature of the result hash.
    pub signature: String,

    /// Attestation timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Task message received from egregore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Task hash (computed by sender).
    pub hash: String,

    /// Task prompt/instruction.
    pub prompt: String,

    /// Required capabilities to execute this task.
    #[serde(default)]
    pub required_caps: Vec<String>,

    /// Parent task ID (for sub-tasks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Additional context.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,

    /// Task priority (higher = more urgent).
    #[serde(default)]
    pub priority: i32,

    /// Task timeout in seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Notification message for outbound communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Servitor sending the notification.
    pub servitor_id: PublicId,

    /// Target channel (e.g., "discord:channel:inbox-alerts").
    pub channel: String,

    /// Priority level.
    pub priority: NotificationPriority,

    /// Notification title.
    pub title: String,

    /// Notification body.
    pub body: String,

    /// Source of the notification.
    pub source: String,

    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for NotificationPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Generic egregore message envelope (for hook input).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgregoreMessage {
    pub author: PublicId,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub content: serde_json::Value,
    pub hash: String,
    pub signature: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl EgregoreMessage {
    /// Extract the content type from the message.
    pub fn content_type(&self) -> Option<&str> {
        self.content.get("type").and_then(|v| v.as_str())
    }

    /// Try to parse content as a Task.
    pub fn as_task(&self) -> Option<Task> {
        if self.content_type() == Some("task") {
            serde_json::from_value(self.content.clone()).ok()
        } else {
            None
        }
    }
}
