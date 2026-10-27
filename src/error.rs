//! Error types for Servitor.

use std::fmt;

use thiserror::Error;

use crate::mcp::RegisteredToolId;
use crate::scope::ScopeRule;

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

    #[error("Scope violation: {diagnostic}")]
    ScopeViolation {
        rule: ScopeRule,
        diagnostic: LocalDiagnostic,
    },

    #[error("tool call {call_index} rejected: {diagnostic}")]
    CallRejected {
        call_index: usize,
        rejection: CallRejection,
        diagnostic: LocalDiagnostic,
    },

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

    #[error("Internal error: {reason}")]
    Internal { reason: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, ServitorError>;

/// Why a planned tool call was refused before it ran, named only by public
/// metadata: the registered tool and the scope rule, never argument values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallRejection {
    /// The call names no tool registered in the MCP pool.
    UnknownTool,
    /// The requesting keeper lacks the skill for this tool.
    Unauthorized { tool: RegisteredToolId },
    /// A scope rule refused the call.
    Scope {
        tool: RegisteredToolId,
        rule: ScopeRule,
    },
}

/// Detailed error text for the operator's local log.
///
/// Construction runs the text through the credential scanner, which is
/// best-effort, so this never reaches the feed: it has no `Serialize`, and
/// feed messages carry closed reason codes instead.
#[derive(Debug, Clone)]
pub struct LocalDiagnostic(String);

impl LocalDiagnostic {
    pub fn new(text: &str) -> Self {
        Self(crate::agent::sanitize::redact_credential_values(text))
    }
}

impl fmt::Display for LocalDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<thallus_core::CoreError> for ServitorError {
    fn from(err: thallus_core::CoreError) -> Self {
        match err {
            thallus_core::CoreError::IdentityNotFound { path } => {
                ServitorError::IdentityNotFound { path }
            }
            thallus_core::CoreError::InvalidKeypair { reason } => {
                ServitorError::InvalidKeypair { reason }
            }
            thallus_core::CoreError::Mcp { reason } => ServitorError::Mcp { reason },
            thallus_core::CoreError::McpServerNotFound { name } => {
                ServitorError::McpServerNotFound { name }
            }
            thallus_core::CoreError::McpValidation { tool, reason } => {
                ServitorError::McpValidation { tool, reason }
            }
            thallus_core::CoreError::Provider { reason } => ServitorError::Provider { reason },
            thallus_core::CoreError::Config { reason } => ServitorError::Config { reason },
            thallus_core::CoreError::Io(e) => ServitorError::Io(e),
            thallus_core::CoreError::Json(e) => ServitorError::Json(e),
            thallus_core::CoreError::Http(e) => ServitorError::Http(e),
        }
    }
}
