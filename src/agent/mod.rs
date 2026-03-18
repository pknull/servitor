//! Agent execution layer.
//!
//! Provides the core task execution loop:
//! - LLM provider abstraction (Anthropic, OpenAI-compatible)
//! - Conversation context management
//! - Tool use → execute → feed back cycle

pub mod context;
pub mod r#loop;
pub mod provider;
pub mod sanitize;

pub use context::ConversationContext;
pub use provider::{create_provider, ChatResponse, ContentBlock, Message, Provider, Role};
pub use r#loop::AgentExecutor;
