//! Servitor — Egregore network task executor using MCP servers as capabilities.
//!
//! Servitor implements the ZeroClaw pattern: it owns MCP clients directly,
//! uses an LLM for reasoning, and publishes signed attestations to egregore.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────────────────────────────────┐
//! │    Egregore     │────▶│                  SERVITOR                    │
//! │   (messages)    │◀────│  ┌─────────────┐  ┌─────────────────────┐  │
//! │                 │     │  │ Task State  │  │   MCP Client Pool   │  │
//! │  - task         │     │  │ (reasoning) │──│  ┌─────┐ ┌─────┐   │  │
//! │  - task_offer   │     │  └─────────────┘  │  │stdio│ │http │   │  │
//! │  - task_assign  │     │                    │  └──┬──┘ └──┬──┘   │  │
//! │  - task_result  │     │                    │  └──┬──┘ └──┬──┘   │  │
//! │  - profile      │     │                    └─────┼───────┼──────┘  │
//! └─────────────────┘     │   ┌──────────────────────┴───────┴──────┐  │
//!                         │   │          Scope Enforcer             │  │
//!                         │   │   (allowlist/blocklist per MCP)     │  │
//!                         │   └─────────────────────────────────────┘  │
//!                         └─────────────────────────────────────────────┘
//! ```
//!
//! ## Three-Plane Model
//!
//! | Plane | Purpose | Examples |
//! |-------|---------|----------|
//! | Communication | Message transport | Egregore, Discord, TUI |
//! | Tool | Execution capabilities | MCP servers, A2A agents |
//! | LLM | Inference/reasoning | Claude, Ollama, OpenAI |
//!
//! ## Event Sources
//!
//! Servitor can receive tasks from multiple sources:
//! - **Cron**: Scheduled tasks via cron expressions
//! - **SSE**: Egregore feed subscription (real-time)
//! - **MCP Notifications**: Server-pushed events
//! - **Hook**: Stdin JSON (for egregore hook mode)
//! - **Direct**: CLI exec command

pub mod a2a;
pub mod agent;
pub mod authority;
pub mod cli;
pub mod comms;
pub mod config;
pub mod egregore;
pub mod error;
pub mod events;
pub mod identity;
pub mod mcp;
pub mod metrics;
pub mod runtime;
pub mod scope;
pub mod task;

pub use authority::{
    authorize_local_exec, load_runtime_authority, AuthRequest, AuthResult, Authority, Keeper,
    PersonId,
};
pub use config::Config;
pub use error::{Result, ServitorError};
pub use identity::{Identity, PublicId};
