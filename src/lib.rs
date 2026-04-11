//! Servitor — Egregore network task executor using MCP servers as capabilities.
//!
//! Servitor is a pure tool executor: it receives tasks with pre-planned tool calls,
//! executes them against MCP servers, and publishes signed attestations to egregore.
//! All reasoning and task decomposition is handled by familiar.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────────────────────────────────┐
//! │    Egregore     │────▶│                  SERVITOR                    │
//! │   (messages)    │◀────│                                              │
//! │                 │     │  ┌─────────────────────┐                    │
//! │  - task         │     │  │   MCP Client Pool   │                    │
//! │  - task_offer   │     │  │  ┌─────┐ ┌─────┐   │                    │
//! │  - task_assign  │     │  │  │stdio│ │http │   │                    │
//! │  - task_result  │     │  │  └──┬──┘ └──┬──┘   │                    │
//! │  - profile      │     │  └─────┼───────┼──────┘                    │
//! └─────────────────┘     │   ┌────┴───────┴────────────────────────┐  │
//!                         │   │          Scope Enforcer             │  │
//!                         │   │   (allowlist/blocklist per MCP)     │  │
//!                         │   └─────────────────────────────────────┘  │
//!                         └─────────────────────────────────────────────┘
//! ```
//!
//! ## Two-Plane Model
//!
//! | Plane | Purpose | Examples |
//! |-------|---------|----------|
//! | Communication | Message transport | Egregore, A2A |
//! | Tool | Execution capabilities | MCP servers, A2A agents |
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
pub mod config;
pub mod egregore;
pub mod error;
pub mod events;
pub mod identity;
pub mod mcp;
pub mod metrics;
pub mod runtime;
pub mod scope;
pub mod session;
pub mod task;

pub use authority::{
    authorize_local_exec, load_runtime_authority, AuthRequest, AuthResult, Authority, Keeper,
    PersonId,
};
pub use config::Config;
pub use error::{Result, ServitorError};
pub use identity::{Identity, PublicId};
