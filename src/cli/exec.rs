//! Direct task execution command.

use std::path::PathBuf;

use crate::a2a::A2aPool;
use crate::authority::{authorize_local_exec, load_runtime_authority};
use crate::config::Config;
use crate::egregore::{EgregoreClient, PlannedToolCall, Task};
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Execute a task directly with pre-planned tool calls.
///
/// Accepts a JSON string containing tool_calls for immediate execution
/// against the local MCP server pool. No LLM reasoning is involved.
pub async fn run_exec(config: &Config, input: &str, insecure: bool) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;
    let authority = load_runtime_authority(&identity_dir, insecure)?;
    let keeper_name = authorize_local_exec(&authority, &identity)?;
    let egregore = EgregoreClient::new(&config.egregore.api_url);

    tracing::info!(id = %identity.public_id(), "executing task");

    // Parse tool calls from JSON input
    let tool_calls: Vec<PlannedToolCall> =
        serde_json::from_str(input).map_err(|e| ServitorError::Config {
            reason: format!(
                "exec requires JSON array of tool calls, e.g. \
                 '[{{\"name\": \"shell__execute\", \"arguments\": {{\"command\": \"ls\"}}}}]'. \
                 Parse error: {}",
                e
            ),
        })?;

    if tool_calls.is_empty() {
        return Err(ServitorError::Config {
            reason: "tool_calls array must not be empty".into(),
        });
    }

    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    // Initialize A2A pool
    let mut a2a_pool = A2aPool::from_config(config).map_err(|e| ServitorError::Config {
        reason: format!("failed to create A2A pool: {}", e),
    })?;
    if !a2a_pool.is_empty() {
        a2a_pool
            .initialize_all()
            .await
            .map_err(|e| ServitorError::Config {
                reason: format!("failed to initialize A2A pool: {}", e),
            })?;
    }

    let mut scope_enforcer = ScopeEnforcer::new();
    for (name, mcp_config) in &config.mcp {
        scope_enforcer.add_policy(name, &mcp_config.scope)?;
    }
    for (name, a2a_config) in &config.a2a {
        scope_enforcer.add_policy(name, &a2a_config.scope)?;
    }

    // Build task with pre-planned tool calls
    let task = Task {
        msg_type: "task".to_string(),
        id: None,
        hash: task_hash(&tool_calls),
        task_type: None,
        request: None,
        requestor: None,
        prompt: format!("exec: {} tool call(s)", tool_calls.len()),
        required_caps: vec![],
        parent_id: None,
        context: std::collections::HashMap::new(),
        scope_override: None,
        priority: 0,
        timeout_secs: Some(config.agent.timeout_secs),
        author: None,
        keeper: keeper_name,
        tool_calls,
        depends_on: vec![],
    };

    let result = crate::agent::direct::execute_direct(
        &task,
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
        Some(&egregore),
        Some(&authority),
        task.keeper.as_deref(),
    )
    .await?;

    // Print result
    println!("Status: {:?}", result.status);
    if let Some(ref r) = result.result {
        println!(
            "Result: {}",
            serde_json::to_string_pretty(r).unwrap_or_default()
        );
    }
    if let Some(ref e) = result.error {
        println!("Error: {}", e);
    }

    // Cleanup
    mcp_pool.shutdown_all().await?;

    Ok(())
}

/// Generate a hash for a set of tool calls.
fn task_hash(tool_calls: &[PlannedToolCall]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for call in tool_calls {
        hasher.update(call.name.as_bytes());
        hasher.update(
            serde_json::to_string(&call.arguments)
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    hasher.update(chrono::Utc::now().timestamp().to_le_bytes());
    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
