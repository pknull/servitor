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
//! │                 │     │  │ Agent Loop  │  │   MCP Client Pool   │  │
//! │  - task         │     │  │ (reasoning) │──│  ┌─────┐ ┌─────┐   │  │
//! │  - task_claim   │     │  └─────────────┘  │  │stdio│ │http │   │  │
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
//! | Tool | Execution capabilities | MCP servers (Docker, Shell) |
//! | LLM | Inference/reasoning | Claude, Ollama, OpenAI |

pub mod agent;
pub mod config;
pub mod egregore;
pub mod error;
pub mod identity;
pub mod mcp;
pub mod scope;

pub use config::Config;
pub use error::{Result, ServitorError};
pub use identity::{Identity, PublicId};
