//! Agent execution layer — direct tool call execution only.
//!
//! Servitors execute pre-planned tool calls against MCP servers.
//! All reasoning and task decomposition is handled by familiar.

pub mod direct;
pub mod output_defense;
pub mod sanitize;

pub use direct::execute_direct;
