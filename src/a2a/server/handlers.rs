//! HTTP handlers for the A2A server.
//!
//! Implements JSON-RPC 2.0 handlers for A2A protocol methods:
//! - tasks/send: Create and start executing a new task
//! - tasks/get: Get current task status
//! - tasks/cancel: Cancel a running task

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::card::build_agent_card;
use super::state::A2aTaskStore;
use crate::a2a::client::TaskResult;
use crate::a2a::A2aPool;
use crate::authority::{AuthRequest, Authority, PersonId};
use crate::config::A2aServerConfig;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Shared state for A2A server handlers.
pub struct A2aServerState {
    pub config: A2aServerConfig,
    pub mcp_pool: Arc<RwLock<McpPool>>,
    pub a2a_pool: Arc<RwLock<A2aPool>>,
    pub authority: Arc<Authority>,
    pub scope_enforcer: Arc<ScopeEnforcer>,
    pub task_store: A2aTaskStore,
    pub base_url: String,
}

/// JSON-RPC request envelope.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC response envelope.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorResponse>,
}

/// JSON-RPC error response.
#[derive(Debug, Serialize)]
pub struct JsonRpcErrorResponse {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcErrorResponse {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// JSON-RPC error codes
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const UNAUTHORIZED: i32 = -32001;
const TASK_NOT_FOUND: i32 = -32002;
const CAPACITY_EXCEEDED: i32 = -32003;

/// Handle GET /.well-known/agent.json
pub async fn handle_agent_card(
    State(state): State<Arc<A2aServerState>>,
) -> impl IntoResponse {
    let mcp_pool = state.mcp_pool.read().await;
    let a2a_pool = state.a2a_pool.read().await;

    let card = build_agent_card(&state.config, &mcp_pool, &a2a_pool, &state.base_url);

    Json(card)
}

/// Handle POST /a2a (JSON-RPC dispatcher)
pub async fn handle_jsonrpc(
    State(state): State<Arc<A2aServerState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // Validate JSON-RPC version
    if request.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::error(
            request.id,
            INVALID_REQUEST,
            "Invalid JSON-RPC version",
        ));
    }

    // Extract bearer token for authentication
    let bearer_token = extract_bearer_token(&headers);

    // Dispatch to method handler
    let response = match request.method.as_str() {
        "tasks/send" => handle_tasks_send(&state, request.id.clone(), request.params, bearer_token).await,
        "tasks/get" => handle_tasks_get(&state, request.id.clone(), request.params).await,
        "tasks/cancel" => handle_tasks_cancel(&state, request.id.clone(), request.params).await,
        _ => JsonRpcResponse::error(
            request.id,
            METHOD_NOT_FOUND,
            format!("Method not found: {}", request.method),
        ),
    };

    Json(response)
}

/// Extract bearer token from Authorization header.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// tasks/send parameters.
#[derive(Debug, Deserialize)]
struct TasksSendParams {
    skill: String,
    #[serde(default)]
    input: serde_json::Value,
}

/// tasks/send response.
#[derive(Debug, Serialize)]
struct TasksSendResponse {
    #[serde(rename = "taskId")]
    task_id: String,
    state: String,
}

/// Handle tasks/send method.
async fn handle_tasks_send(
    state: &A2aServerState,
    id: serde_json::Value,
    params: Option<serde_json::Value>,
    bearer_token: Option<String>,
) -> JsonRpcResponse {
    // Parse params
    let params: TasksSendParams = match params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "Missing params");
        }
    };

    // Authenticate via bearer token
    let keeper_name = if let Some(token) = bearer_token {
        let person = PersonId::from_http(token);
        let auth_result = state.authority.authorize(&AuthRequest {
            person,
            place: "a2a:server".to_string(),
            skill: params.skill.clone(),
        });

        if !auth_result.allowed {
            tracing::warn!(
                skill = %params.skill,
                reason = %auth_result.reason,
                "A2A server request unauthorized"
            );
            return JsonRpcResponse::error(id, UNAUTHORIZED, auth_result.reason);
        }

        auth_result.keeper
    } else if state.authority.is_open_mode() {
        // Open mode allows anonymous requests
        None
    } else {
        return JsonRpcResponse::error(id, UNAUTHORIZED, "Authorization required");
    };

    // Check if skill exists
    let skill_exists = {
        let mcp_pool = state.mcp_pool.read().await;
        let a2a_pool = state.a2a_pool.read().await;
        mcp_pool.parse_tool_name(&params.skill).is_some()
            || a2a_pool.has_tool(&params.skill)
    };

    if !skill_exists {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            format!("Unknown skill: {}", params.skill),
        );
    }

    // Create task
    let task = match state
        .task_store
        .create(params.skill.clone(), params.input.clone(), keeper_name.clone())
        .await
    {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(id, CAPACITY_EXCEEDED, "Task capacity exceeded");
        }
    };

    let task_id = task.id.clone();

    // Spawn async execution
    let state_clone = Arc::new(A2aServerStateRef {
        mcp_pool: state.mcp_pool.clone(),
        a2a_pool: state.a2a_pool.clone(),
        authority: state.authority.clone(),
        scope_enforcer: state.scope_enforcer.clone(),
        task_store: state.task_store.clone(),
    });

    tokio::spawn(async move {
        execute_task(state_clone, task_id.clone(), params.skill, params.input, keeper_name).await;
    });

    JsonRpcResponse::success(
        id,
        serde_json::json!(TasksSendResponse {
            task_id: task.id,
            state: "submitted".to_string(),
        }),
    )
}

/// Lightweight state reference for spawned tasks.
struct A2aServerStateRef {
    mcp_pool: Arc<RwLock<McpPool>>,
    a2a_pool: Arc<RwLock<A2aPool>>,
    #[allow(dead_code)]
    authority: Arc<Authority>,
    scope_enforcer: Arc<ScopeEnforcer>,
    task_store: A2aTaskStore,
}

/// Execute a task asynchronously.
async fn execute_task(
    state: Arc<A2aServerStateRef>,
    task_id: String,
    skill: String,
    input: serde_json::Value,
    _keeper_name: Option<String>,
) {
    // Mark as working
    state.task_store.start(&task_id).await;

    tracing::info!(task_id = %task_id, skill = %skill, "executing A2A task");

    // Check scope enforcement (always, regardless of auth status)
    if let Some(underscore_pos) = skill.find('_') {
        let server_name = &skill[..underscore_pos];
        let tool_name = &skill[underscore_pos + 1..];
        if let Err(e) = state.scope_enforcer.check(server_name, tool_name, &input, None) {
            let error = format!("Scope violation: {}", e);
            tracing::warn!(task_id = %task_id, error = %error, "task scope violation");
            state.task_store.fail(&task_id, error).await;
            return;
        }
    }

    // Try MCP pool first
    {
        let mcp_pool = state.mcp_pool.read().await;
        if mcp_pool.parse_tool_name(&skill).is_some() {
            match mcp_pool.call_tool(&skill, input.clone()).await {
                Ok(result) => {
                    let task_result = TaskResult {
                        text: Some(result.text_content()),
                        data: None,
                        artifacts: vec![],
                    };
                    state.task_store.complete(&task_id, task_result).await;
                    tracing::info!(task_id = %task_id, "A2A task completed (MCP)");
                    return;
                }
                Err(e) => {
                    let error = format!("MCP tool error: {}", e);
                    tracing::error!(task_id = %task_id, error = %error, "A2A task failed");
                    state.task_store.fail(&task_id, error).await;
                    return;
                }
            }
        }
    }

    // Try A2A pool
    {
        let a2a_pool = state.a2a_pool.read().await;
        if a2a_pool.has_tool(&skill) {
            match a2a_pool.execute_skill(&skill, input).await {
                Ok(result) => {
                    state.task_store.complete(&task_id, result).await;
                    tracing::info!(task_id = %task_id, "A2A task completed (A2A)");
                    return;
                }
                Err(e) => {
                    let error = format!("A2A skill error: {}", e);
                    tracing::error!(task_id = %task_id, error = %error, "A2A task failed");
                    state.task_store.fail(&task_id, error).await;
                    return;
                }
            }
        }
    }

    // Skill not found (shouldn't happen, checked earlier)
    state
        .task_store
        .fail(&task_id, format!("Unknown skill: {}", skill))
        .await;
}

/// tasks/get parameters.
#[derive(Debug, Deserialize)]
struct TasksGetParams {
    #[serde(rename = "taskId")]
    task_id: String,
}

/// Handle tasks/get method.
async fn handle_tasks_get(
    state: &A2aServerState,
    id: serde_json::Value,
    params: Option<serde_json::Value>,
) -> JsonRpcResponse {
    let params: TasksGetParams = match params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "Missing params");
        }
    };

    match state.task_store.get(&params.task_id).await {
        Some(task) => {
            let client_task = task.to_client_task();
            match serde_json::to_value(client_task) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(_) => JsonRpcResponse::error(id, -32603, "Internal serialization error"),
            }
        }
        None => JsonRpcResponse::error(
            id,
            TASK_NOT_FOUND,
            format!("Task not found: {}", params.task_id),
        ),
    }
}

/// tasks/cancel parameters.
#[derive(Debug, Deserialize)]
struct TasksCancelParams {
    #[serde(rename = "taskId")]
    task_id: String,
}

/// Handle tasks/cancel method.
async fn handle_tasks_cancel(
    state: &A2aServerState,
    id: serde_json::Value,
    params: Option<serde_json::Value>,
) -> JsonRpcResponse {
    let params: TasksCancelParams = match params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "Missing params");
        }
    };

    match state.task_store.cancel(&params.task_id).await {
        Some(task) => {
            let client_task = task.to_client_task();
            match serde_json::to_value(client_task) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(_) => JsonRpcResponse::error(id, -32603, "Internal serialization error"),
            }
        }
        None => {
            // Could be not found or already terminal
            match state.task_store.get(&params.task_id).await {
                Some(task) => JsonRpcResponse::error(
                    id,
                    INVALID_REQUEST,
                    format!("Task already in terminal state: {:?}", task.state),
                ),
                None => JsonRpcResponse::error(
                    id,
                    TASK_NOT_FOUND,
                    format!("Task not found: {}", params.task_id),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonrpc_response_serialization() {
        let success = JsonRpcResponse::success(
            serde_json::json!(1),
            serde_json::json!({"taskId": "abc"}),
        );
        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));

        let error = JsonRpcResponse::error(serde_json::json!(1), -32600, "Bad request");
        let json = serde_json::to_string(&error).unwrap();
        assert!(!json.contains("\"result\""));
        assert!(json.contains("\"error\""));
    }

    #[test]
    fn test_extract_bearer_token() {
        let mut headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_none());

        headers.insert("authorization", "Bearer abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), Some("abc123".to_string()));

        headers.insert("authorization", "Basic xyz".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_none());
    }
}
