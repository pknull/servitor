//! A2A client trait and types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::card::AgentCard;
use super::Result;

/// A2A client trait — abstraction for communicating with A2A agents.
#[async_trait]
pub trait A2aClient: Send + Sync {
    /// Fetch and cache the agent's card from the card URL.
    async fn fetch_card(&mut self) -> Result<AgentCard>;

    /// Get the cached agent card (None if not yet fetched).
    fn card(&self) -> Option<&AgentCard>;

    /// Execute a task synchronously (blocks until completion or timeout).
    ///
    /// This internally calls `send_message` and polls `get_task` until
    /// the task reaches a terminal state.
    async fn execute_task(&self, skill: &str, input: serde_json::Value) -> Result<TaskResult>;

    /// Send a message to start a task (non-blocking).
    ///
    /// Returns the task ID for subsequent polling.
    async fn send_message(&self, skill: &str, input: serde_json::Value) -> Result<String>;

    /// Get current task status.
    async fn get_task(&self, task_id: &str) -> Result<A2aTask>;

    /// Cancel a running task.
    async fn cancel_task(&self, task_id: &str) -> Result<()>;

    /// Get the agent name (for tool prefixing).
    fn name(&self) -> &str;
}

/// A2A task state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Task is queued but not yet started.
    Submitted,
    /// Task is currently being processed.
    Working,
    /// Task requires additional input (streaming).
    InputRequired,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Cancelled,
}

impl TaskState {
    /// Check if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// A2A task representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aTask {
    /// Unique task identifier.
    pub id: String,

    /// Current task state.
    pub state: TaskState,

    /// Task result (if completed).
    #[serde(default)]
    pub result: Option<TaskResult>,

    /// Error message (if failed).
    #[serde(default)]
    pub error: Option<String>,

    /// Artifacts produced by the task.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// Task result on successful completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Text output from the task.
    #[serde(default)]
    pub text: Option<String>,

    /// Structured data output.
    #[serde(default)]
    pub data: Option<serde_json::Value>,

    /// Artifacts produced.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

impl TaskResult {
    /// Create a text result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            data: None,
            artifacts: Vec::new(),
        }
    }

    /// Create a data result.
    pub fn data(data: serde_json::Value) -> Self {
        Self {
            text: None,
            data: Some(data),
            artifacts: Vec::new(),
        }
    }

    /// Get displayable content (text or JSON-serialized data).
    pub fn content(&self) -> String {
        if let Some(text) = &self.text {
            text.clone()
        } else if let Some(data) = &self.data {
            serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string())
        } else {
            String::new()
        }
    }

    /// Convert to MCP ToolCallResult for compatibility with agent loop.
    pub fn to_mcp_result(&self) -> crate::mcp::ToolCallResult {
        crate::mcp::ToolCallResult::text(self.content())
    }
}

/// Artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact name.
    pub name: String,

    /// MIME type.
    #[serde(default)]
    pub mime_type: Option<String>,

    /// Artifact data (base64 encoded for binary).
    #[serde(default)]
    pub data: Option<String>,

    /// URI reference (alternative to inline data).
    #[serde(default)]
    pub uri: Option<String>,
}

/// JSON-RPC request for A2A protocol.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response for A2A protocol.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}
