//! MCP client abstraction.
//!
//! Provides the McpClient trait and implementations for:
//! - Stdio transport (subprocess JSON-RPC)
//! - HTTP transport (JSON-RPC over HTTP)
//!
//! The pool manages multiple clients and provides tool introspection
//! with prefixed tool names for LLM consumption.

pub mod client;
pub mod http;
pub mod pool;
pub mod stdio;

pub use client::{McpClient, ToolCallResult, ToolDefinition};
pub use pool::{LlmTool, McpPool};
