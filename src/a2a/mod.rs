//! A2A (Agent-to-Agent) layer.
//!
//! Provides both client and server functionality for A2A protocol:
//!
//! ## Client Layer
//!
//! Integration with A2A-compatible agent services (CrewAI, LangGraph, etc.)
//! allowing Servitor to delegate tasks to external agent pools.
//!
//! ## Server Layer
//!
//! HTTP server that exposes Servitor's MCP tools and A2A skills to external
//! agents, making Servitor a full peer in A2A networks.
//!
//! ## A2A Protocol Overview
//!
//! A2A uses JSON-RPC 2.0 over HTTPS with:
//! - Agent Cards for capability discovery (/.well-known/agent.json)
//! - Async task lifecycle (send → working → completed/failed)
//! - Bearer token authentication
//!
//! ## Tool Integration
//!
//! A2A agents appear as tools with prefixed names:
//! - Agent `researcher` with skill `web_search` → tool `researcher_web_search`

pub mod card;
pub mod client;
pub mod http;
pub mod pool;
pub mod server;

pub use card::{AgentCard, Skill};
pub use client::{A2aClient, A2aTask, TaskResult, TaskState};
pub use http::HttpA2aClient;
pub use pool::A2aPool;

use thiserror::Error;

/// A2A-specific errors.
#[derive(Error, Debug)]
pub enum A2aError {
    #[error("A2A agent '{name}' not found")]
    AgentNotFound { name: String },

    #[error("A2A skill '{skill}' not found on agent '{agent}'")]
    SkillNotFound { agent: String, skill: String },

    #[error("Agent card fetch failed for '{agent}': {reason}")]
    CardFetchFailed { agent: String, reason: String },

    #[error("Task '{task_id}' failed: {reason}")]
    TaskFailed { task_id: String, reason: String },

    #[error("Task '{task_id}' timed out after {seconds}s")]
    TaskTimeout { task_id: String, seconds: u64 },

    #[error("Task '{task_id}' was cancelled")]
    TaskCancelled { task_id: String },

    #[error("A2A protocol error: {reason}")]
    Protocol { reason: String },

    #[error("A2A authentication failed: {reason}")]
    AuthFailed { reason: String },

    #[error("A2A HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("A2A JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, A2aError>;
