//! Error types for Servitor.

use thiserror::Error;

/// Main error type for Servitor operations.
#[derive(Error, Debug)]
pub enum ServitorError {
    #[error("Configuration error: {reason}")]
    Config { reason: String },

    #[error("Identity not found at {path}")]
    IdentityNotFound { path: String },

    #[error("Invalid keypair: {reason}")]
    InvalidKeypair { reason: String },

    #[error("MCP error: {reason}")]
    Mcp { reason: String },

    #[error("MCP server '{name}' not found")]
    McpServerNotFound { name: String },

    #[error("Invalid arguments for MCP tool '{tool}': {reason}")]
    McpValidation { tool: String, reason: String },

    #[error("Scope violation: {reason}")]
    ScopeViolation { reason: String },

    #[error("LLM provider error: {reason}")]
    Provider { reason: String },

    #[error("Egregore API error: {reason}")]
    Egregore { reason: String },

    #[error("Task execution error: {reason}")]
    TaskExecution { reason: String },

    #[error("Timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("Cron expression error: {reason}")]
    Cron { reason: String },

    #[error("SSE connection error: {reason}")]
    Sse { reason: String },

    #[error("Communication transport error: {reason}")]
    Comms { reason: String },

    #[error("Authorization denied: {reason}")]
    Unauthorized { reason: String },

    #[error("Plan validation failed: {reason}")]
    PlanValidation { reason: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, ServitorError>;
