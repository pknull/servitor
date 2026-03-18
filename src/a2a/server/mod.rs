//! A2A server module for receiving tasks from external agents.
//!
//! This module implements an HTTP server that exposes servitor's capabilities
//! via the A2A protocol, allowing external agents to discover and invoke
//! MCP tools and A2A skills.
//!
//! ## Endpoints
//!
//! - `GET /.well-known/agent.json` - Agent card (capability discovery)
//! - `POST /a2a` - JSON-RPC 2.0 endpoint for task operations
//!
//! ## Authentication
//!
//! Uses Bearer token authentication mapped to PersonId::Http for
//! integration with the existing Authority system.

mod card;
mod handlers;
mod routes;
mod state;

pub use card::build_agent_card;
pub use handlers::A2aServerState;
pub use routes::build_router;
pub use state::{A2aServerTask, A2aTaskStore};

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::RwLock;

use crate::a2a::A2aPool;
use crate::authority::Authority;
use crate::config::A2aServerConfig;
use crate::error::{Result, ServitorError};
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Spawn the A2A server as a background task.
///
/// Returns a handle that can be used to await completion (or error).
pub async fn spawn_server(
    config: A2aServerConfig,
    mcp_pool: Arc<RwLock<McpPool>>,
    a2a_pool: Arc<RwLock<A2aPool>>,
    authority: Arc<Authority>,
    scope_enforcer: Arc<ScopeEnforcer>,
) -> Result<tokio::task::JoinHandle<Result<()>>> {
    if !config.enabled {
        tracing::debug!("A2A server disabled in config");
        // Return a handle that completes immediately
        return Ok(tokio::spawn(async { Ok(()) }));
    }

    let bind_addr = config.bind.clone();
    let base_url = format!("http://{}", bind_addr);

    let task_store = A2aTaskStore::new(
        config.max_concurrent_tasks,
        config.task_timeout_secs,
    );

    let state = Arc::new(A2aServerState {
        config,
        mcp_pool,
        a2a_pool,
        authority,
        scope_enforcer,
        task_store,
        base_url,
    });

    let router = build_router(state);

    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        ServitorError::Config {
            reason: format!("failed to bind A2A server to {}: {}", bind_addr, e),
        }
    })?;

    tracing::info!(bind = %bind_addr, "A2A server listening");

    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .map_err(|e| ServitorError::Internal {
                reason: format!("A2A server error: {}", e),
            })
    });

    Ok(handle)
}
