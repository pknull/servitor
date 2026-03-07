//! MCP (Model Context Protocol) client layer.
//!
//! Provides a unified interface for communicating with MCP servers
//! over stdio (subprocess) and HTTP transports.

pub mod client;
pub mod http;
pub mod pool;
pub mod stdio;

pub use client::{McpClient, ToolCallResult, ToolContent, ToolDefinition};
pub use pool::{LlmTool, McpPool};
