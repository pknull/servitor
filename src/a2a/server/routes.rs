//! Axum router setup for the A2A server.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use super::handlers::{handle_agent_card, handle_jsonrpc, A2aServerState};

/// Build the Axum router for the A2A server.
pub fn build_router(state: Arc<A2aServerState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/.well-known/agent.json", get(handle_agent_card))
        .route("/a2a", post(handle_jsonrpc))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
