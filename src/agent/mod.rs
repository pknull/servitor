//! Agent loop orchestration.
//!
//! The agent loop handles:
//! 1. Receiving tasks from egregore
//! 2. Planning phase (capability checking)
//! 3. LLM conversation with tool_use blocks
//! 4. Tool execution via MCP clients
//! 5. Result aggregation and attestation signing
//! 6. Publishing results back to egregore

pub mod context;
pub mod r#loop;
pub mod provider;

pub use context::ConversationContext;
pub use provider::{create_provider, ChatResponse, ContentBlock, Message, Provider, StopReason};
pub use r#loop::AgentExecutor;
