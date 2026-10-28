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
use super::state::{A2aServerTask, A2aTaskStore, CancelOutcome};
use crate::a2a::client::TaskResult;
use crate::a2a::A2aPool;
use crate::authority::{AuthRequest, Authority, PersonId};
use crate::config::A2aServerConfig;
use crate::error::ServitorError;
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
pub async fn handle_agent_card(State(state): State<Arc<A2aServerState>>) -> impl IntoResponse {
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
        "tasks/send" => {
            handle_tasks_send(&state, request.id.clone(), request.params, bearer_token).await
        }
        "tasks/get" => {
            handle_tasks_get(&state, request.id.clone(), request.params, bearer_token).await
        }
        "tasks/cancel" => {
            handle_tasks_cancel(&state, request.id.clone(), request.params, bearer_token).await
        }
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

/// A2A message part.
#[derive(Debug, Deserialize)]
struct MessagePart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// A2A message format.
#[derive(Debug, Deserialize)]
struct A2aMessage {
    #[serde(default)]
    parts: Vec<MessagePart>,
}

/// tasks/send parameters.
#[derive(Debug, Deserialize)]
struct TasksSendParams {
    skill: String,
    /// Direct JSON input (preferred for internal use).
    #[serde(default)]
    input: serde_json::Value,
    /// A2A message format (for A2A protocol compatibility).
    #[serde(default)]
    message: Option<A2aMessage>,
}

impl TasksSendParams {
    /// Extract the input value, preferring direct input over message format.
    fn extract_input(&self) -> serde_json::Value {
        // If direct input is provided and not null, use it
        if !self.input.is_null() {
            return self.input.clone();
        }

        // Try to extract from message format
        if let Some(ref message) = self.message {
            for part in &message.parts {
                if part.part_type == "text" {
                    if let Some(ref text) = part.text {
                        // Try to parse the text as JSON
                        if let Ok(parsed) = serde_json::from_str(text) {
                            return parsed;
                        }
                    }
                }
            }
        }

        // Default to null
        serde_json::Value::Null
    }
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

    // Open mode allows anonymous requests; otherwise a bearer token is required
    if bearer_token.is_none() && !state.authority.is_open_mode() {
        return JsonRpcResponse::error(id, UNAUTHORIZED, "Authorization required");
    }

    let Some((provider, tool)) =
        registered_skill(&state.mcp_pool, &state.a2a_pool, &params.skill).await
    else {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            format!("Unknown skill: {}", params.skill),
        );
    };

    // Authenticate via bearer token, authorizing the registered provider:tool
    // as direct execution does
    let keeper_name = match bearer_token {
        Some(token) => {
            let auth_result = state.authority.authorize(&AuthRequest {
                person: PersonId::from_http(token),
                skill: format!("{provider}:{tool}"),
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
        }
        None => None,
    };

    // Extract input from either direct input or message format
    let input = params.extract_input();

    // Create task
    let task = match state
        .task_store
        .create(params.skill.clone(), input.clone(), keeper_name.clone())
        .await
    {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(id, CAPACITY_EXCEEDED, "Task capacity exceeded");
        }
    };

    let task_id = task.id.clone();
    let skill = params.skill.clone();

    // Spawn async execution
    let state_clone = Arc::new(A2aServerStateRef {
        mcp_pool: state.mcp_pool.clone(),
        a2a_pool: state.a2a_pool.clone(),
        authority: state.authority.clone(),
        scope_enforcer: state.scope_enforcer.clone(),
        task_store: state.task_store.clone(),
    });

    tokio::spawn(async move {
        execute_task(state_clone, task_id.clone(), skill, input, keeper_name).await;
    });

    JsonRpcResponse::success(
        id,
        serde_json::json!(TasksSendResponse {
            task_id: task.id,
            state: "submitted".to_string(),
        }),
    )
}

/// Resolve an advertised skill name to its registered provider and tool.
///
/// Authority and scope both key on this identity, never on a split of the
/// skill name, which would misplace providers whose names hold an
/// underscore. The MCP pool is consulted first, as execution does.
async fn registered_skill(
    mcp_pool: &RwLock<McpPool>,
    a2a_pool: &RwLock<A2aPool>,
    skill: &str,
) -> Option<(String, String)> {
    let mcp_pool = mcp_pool.read().await;
    let a2a_pool = a2a_pool.read().await;
    mcp_pool
        .parse_tool_name(skill)
        .or_else(|| a2a_pool.parse_tool_name(skill))
        .map(|(provider, tool)| (provider.to_string(), tool.to_string()))
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
    let Some((provider, tool)) = registered_skill(&state.mcp_pool, &state.a2a_pool, &skill).await
    else {
        state
            .task_store
            .fail(&task_id, format!("Unknown skill: {}", skill))
            .await;
        return;
    };
    if let Err(e) = state.scope_enforcer.check(&provider, &tool, &input, None) {
        tracing::warn!(task_id = %task_id, error = %e, "task scope violation");
        // The client learns which rule refused the call; the diagnostic,
        // which quotes the matched pattern, stays in the local log.
        let error = match e {
            ServitorError::ScopeViolation { rule, .. } => format!("Scope violation: {rule}"),
            _ => "Scope check failed".to_string(),
        };
        state.task_store.fail(&task_id, error).await;
        return;
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

/// Who a task read or cancel acts for.
enum TaskAccess {
    /// Open mode: every task is reachable.
    Open,
    /// An identified keeper reaches only its own tasks.
    Keeper(String),
}

impl TaskAccess {
    /// Resolve the caller. Outside open mode the bearer token must identify
    /// a configured keeper.
    fn for_request(state: &A2aServerState, bearer_token: Option<String>) -> Result<Self, &str> {
        if state.authority.is_open_mode() {
            return Ok(Self::Open);
        }
        let Some(token) = bearer_token else {
            return Err("Authorization required");
        };
        state
            .authority
            .identify(&PersonId::from_http(token))
            .map(|keeper| Self::Keeper(keeper.name.clone()))
            .ok_or("Unknown identity")
    }

    fn reaches(&self, task: &A2aServerTask) -> bool {
        match self {
            Self::Open => true,
            Self::Keeper(name) => task.keeper.as_deref() == Some(name.as_str()),
        }
    }
}

/// Handle tasks/get method.
async fn handle_tasks_get(
    state: &A2aServerState,
    id: serde_json::Value,
    params: Option<serde_json::Value>,
    bearer_token: Option<String>,
) -> JsonRpcResponse {
    let access = match TaskAccess::for_request(state, bearer_token) {
        Ok(access) => access,
        Err(reason) => return JsonRpcResponse::error(id, UNAUTHORIZED, reason),
    };

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

    // Another keeper's task reads as not found
    match state
        .task_store
        .get_reachable(&params.task_id, |task| access.reaches(task))
        .await
    {
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
    bearer_token: Option<String>,
) -> JsonRpcResponse {
    let access = match TaskAccess::for_request(state, bearer_token) {
        Ok(access) => access,
        Err(reason) => return JsonRpcResponse::error(id, UNAUTHORIZED, reason),
    };

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

    // Another keeper's task reads as not found and stays untouched
    match state
        .task_store
        .cancel(&params.task_id, |task| access.reaches(task))
        .await
    {
        CancelOutcome::Cancelled(task) => {
            let client_task = task.to_client_task();
            match serde_json::to_value(client_task) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(_) => JsonRpcResponse::error(id, -32603, "Internal serialization error"),
            }
        }
        CancelOutcome::Terminal(task_state) => JsonRpcResponse::error(
            id,
            INVALID_REQUEST,
            format!("Task already in terminal state: {:?}", task_state),
        ),
        CancelOutcome::NotFound => JsonRpcResponse::error(
            id,
            TASK_NOT_FOUND,
            format!("Task not found: {}", params.task_id),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::client::{A2aTask, TaskState};
    use crate::config::ScopeConfig;
    use crate::test_support::ScriptedClient;

    /// Run one task for `server`'s `execute` tool, under the given block
    /// rules, through the server's execution path, and return what
    /// `tasks/get` would hand the client.
    async fn run_scoped_task(server: &str, block: &[&str], input: serde_json::Value) -> A2aTask {
        let mut mcp_pool = McpPool::new();
        mcp_pool.register_test_tool(server, "execute", ScriptedClient::output("ran"));
        let mut scope_enforcer = ScopeEnforcer::new();
        scope_enforcer
            .add_policy(
                server,
                &ScopeConfig {
                    allow: vec![],
                    block: block.iter().map(|rule| rule.to_string()).collect(),
                },
            )
            .unwrap();
        let task_store = A2aTaskStore::new(8, 60);
        let skill = format!("{server}_execute");
        let task = task_store
            .create(skill.clone(), input.clone(), None)
            .await
            .unwrap();
        let state = Arc::new(A2aServerStateRef {
            mcp_pool: Arc::new(RwLock::new(mcp_pool)),
            a2a_pool: Arc::new(RwLock::new(A2aPool::new())),
            authority: Arc::new(Authority::insecure_open()),
            scope_enforcer: Arc::new(scope_enforcer),
            task_store: task_store.clone(),
        });
        execute_task(state, task.id.clone(), skill, input, None).await;
        task_store.get(&task.id).await.unwrap().to_client_task()
    }

    #[tokio::test]
    async fn scope_violation_names_the_rule_without_its_pattern() {
        let task = run_scoped_task(
            "shell",
            &["execute:/srv/sentinel-pattern-*"],
            serde_json::json!({ "command": "/srv/sentinel-pattern-run" }),
        )
        .await;

        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(
            task.error.as_deref(),
            Some("Scope violation: blocked by policy block rule 0")
        );
        let reply = serde_json::to_string(&task).unwrap();
        assert!(
            !reply.contains("sentinel-pattern-*"),
            "tasks/get returns the block pattern: {reply}"
        );
    }

    #[tokio::test]
    async fn scope_policy_applies_to_servers_whose_names_hold_an_underscore() {
        let task = run_scoped_task(
            "local_shell",
            &["execute:/srv/*"],
            serde_json::json!({ "command": "/srv/run" }),
        )
        .await;

        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(
            task.error.as_deref(),
            Some("Scope violation: blocked by policy block rule 0")
        );
    }

    /// A restricted server offering `execute` on each provider, with keepers
    /// `operator` and `other` holding bearer tokens `operator-token` and
    /// `other-token` and the given skills.
    fn restricted_server(providers: &[&str], skills: &[&str]) -> A2aServerState {
        let mut mcp_pool = McpPool::new();
        for provider in providers {
            mcp_pool.register_test_tool(provider, "execute", ScriptedClient::output("ran"));
        }
        let authority = Authority::from_config(
            crate::authority::AuthorityConfig::from_toml(&format!(
                r#"
[[keeper]]
name = "operator"
http_token = "operator-token"

[[keeper]]
name = "other"
http_token = "other-token"

[[permission]]
keeper = "operator"
skills = {skills:?}

[[permission]]
keeper = "other"
skills = {skills:?}
"#
            ))
            .unwrap(),
        );
        A2aServerState {
            config: A2aServerConfig::default(),
            mcp_pool: Arc::new(RwLock::new(mcp_pool)),
            a2a_pool: Arc::new(RwLock::new(A2aPool::new())),
            authority: Arc::new(authority),
            scope_enforcer: Arc::new(ScopeEnforcer::new()),
            task_store: A2aTaskStore::new(8, 60),
            base_url: "http://127.0.0.1:0".to_string(),
        }
    }

    async fn send(state: &A2aServerState, skill: &str, token: Option<&str>) -> JsonRpcResponse {
        handle_tasks_send(
            state,
            serde_json::json!(1),
            Some(serde_json::json!({ "skill": skill, "input": {} })),
            token.map(str::to_string),
        )
        .await
    }

    #[tokio::test]
    async fn skill_permissions_match_the_registered_provider_and_tool() {
        let state = restricted_server(&["shell"], &["shell:*"]);
        let response = send(&state, "shell_execute", Some("operator-token")).await;
        assert!(
            response.error.is_none(),
            "shell:* must authorize shell's execute tool: {:?}",
            response.error.map(|e| e.message)
        );
    }

    #[tokio::test]
    async fn underscore_permission_does_not_reach_another_provider() {
        let state = restricted_server(&["shell", "shell_admin"], &["shell_*"]);
        let response = send(&state, "shell_admin_execute", Some("operator-token")).await;
        assert_eq!(response.error.map(|e| e.code), Some(UNAUTHORIZED));
    }

    /// Call a JSON-RPC method through the dispatcher and return the response
    /// body.
    async fn call(
        state: &Arc<A2aServerState>,
        method: &str,
        params: serde_json::Value,
        token: Option<&str>,
    ) -> serde_json::Value {
        let mut headers = HeaderMap::new();
        if let Some(token) = token {
            headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: method.to_string(),
            params: Some(params),
        };
        let response = handle_jsonrpc(State(state.clone()), headers, Json(request))
            .await
            .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// A restricted server holding one submitted task owned by `operator`.
    async fn server_with_operator_task() -> (Arc<A2aServerState>, String) {
        let state = restricted_server(&["shell"], &["*"]);
        let task = state
            .task_store
            .create(
                "shell_execute".to_string(),
                serde_json::json!({}),
                Some("operator".to_string()),
            )
            .await
            .unwrap();
        (Arc::new(state), task.id)
    }

    #[tokio::test]
    async fn task_reads_and_cancels_require_a_known_keeper() {
        let (state, task_id) = server_with_operator_task().await;
        for method in ["tasks/get", "tasks/cancel"] {
            for token in [None, Some("unknown-token")] {
                let reply = call(
                    &state,
                    method,
                    serde_json::json!({ "taskId": task_id }),
                    token,
                )
                .await;
                assert_eq!(
                    reply["error"]["code"], UNAUTHORIZED,
                    "{method} with token {token:?}: {reply}"
                );
            }
        }
        let task = state.task_store.get(&task_id).await.unwrap();
        assert_eq!(task.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn another_keepers_task_reads_as_not_found() {
        let (state, task_id) = server_with_operator_task().await;
        let missing_id = "00000000-0000-4000-8000-000000000000";
        for method in ["tasks/get", "tasks/cancel"] {
            let reply = call(
                &state,
                method,
                serde_json::json!({ "taskId": task_id }),
                Some("other-token"),
            )
            .await;
            assert_eq!(reply["error"]["code"], TASK_NOT_FOUND, "{method}: {reply}");
            // The reply is the one a missing task gets, apart from the ID.
            let missing = call(
                &state,
                method,
                serde_json::json!({ "taskId": missing_id }),
                Some("other-token"),
            )
            .await;
            assert_eq!(
                reply.to_string().replace(&task_id, "ID"),
                missing.to_string().replace(missing_id, "ID"),
                "{method}"
            );
        }
        let task = state.task_store.get(&task_id).await.unwrap();
        assert_eq!(task.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn the_owning_keeper_reads_and_cancels_its_task() {
        let (state, task_id) = server_with_operator_task().await;
        let params = serde_json::json!({ "taskId": task_id });
        let reply = call(&state, "tasks/get", params.clone(), Some("operator-token")).await;
        assert_eq!(reply["result"]["id"], task_id.as_str(), "{reply}");
        let reply = call(&state, "tasks/cancel", params, Some("operator-token")).await;
        assert_eq!(reply["result"]["state"], "cancelled", "{reply}");
    }

    #[tokio::test]
    async fn open_mode_reads_and_cancels_tasks_without_a_token() {
        let task_store = A2aTaskStore::new(8, 60);
        let task = task_store
            .create("shell_execute".to_string(), serde_json::json!({}), None)
            .await
            .unwrap();
        let state = Arc::new(A2aServerState {
            config: A2aServerConfig::default(),
            mcp_pool: Arc::new(RwLock::new(McpPool::new())),
            a2a_pool: Arc::new(RwLock::new(A2aPool::new())),
            authority: Arc::new(Authority::insecure_open()),
            scope_enforcer: Arc::new(ScopeEnforcer::new()),
            task_store,
            base_url: "http://127.0.0.1:0".to_string(),
        });
        let params = serde_json::json!({ "taskId": task.id });
        let reply = call(&state, "tasks/get", params.clone(), None).await;
        assert_eq!(reply["result"]["id"], task.id.as_str(), "{reply}");
        let reply = call(&state, "tasks/cancel", params, None).await;
        assert_eq!(reply["result"]["state"], "cancelled", "{reply}");
    }

    #[tokio::test]
    async fn anonymous_request_is_refused_before_skill_lookup() {
        let state = restricted_server(&["shell"], &["*"]);
        let response = send(&state, "no_such_skill", None).await;
        let error = response.error.unwrap();
        assert_eq!(error.code, UNAUTHORIZED);
        assert_eq!(error.message, "Authorization required");
    }

    #[test]
    fn test_jsonrpc_response_serialization() {
        let success =
            JsonRpcResponse::success(serde_json::json!(1), serde_json::json!({"taskId": "abc"}));
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
