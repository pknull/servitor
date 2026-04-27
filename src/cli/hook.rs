//! Hook mode implementation for egregore integration.

use std::path::PathBuf;

use crate::a2a::A2aPool;
use crate::authority::{load_runtime_authority, PersonId};
use crate::config::Config;
use crate::egregore::{AuthGate, EgregoreClient, TaskClaim, TaskFailed, TaskFailureReason};
use crate::error::Result;
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::metrics::{self, AuthDecision};
use crate::runtime::publish_auth_denied_event;
use crate::scope::ScopeEnforcer;
use crate::task::{authorize_offer_request, inherit_trace_context, request_skill};

/// Run servitor in hook mode (stdin JSON from egregore).
///
/// This mode reads a single task from stdin, executes pre-planned tool calls,
/// and publishes the result to the egregore network. Designed to be invoked
/// by the egregore daemon as a hook handler.
pub async fn run_hook(config: &Config, insecure: bool) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;
    let egregore = EgregoreClient::new(&config.egregore.api_url);

    // Load authority
    let authority = load_runtime_authority(&identity_dir, insecure)?;

    tracing::info!(id = %identity.public_id(), "starting hook mode");

    // Parse incoming message from stdin
    let message = match crate::egregore::hook::receive_message() {
        Ok(msg) => msg,
        Err(e) => {
            tracing::error!(error = %e, "failed to receive message");
            return Err(e);
        }
    };

    // Extract task from message
    let mut task = message
        .as_task()
        .ok_or_else(|| crate::ServitorError::Egregore {
            reason: "message is not a task".into(),
        })?;
    task.author = Some(message.author.0.clone());
    task.normalize(Some(&message.author));
    inherit_trace_context(&mut task, &message);

    let Some(requestor) = task.requestor.clone() else {
        tracing::warn!(task_id = %task.effective_id(), "ignoring task without requestor");
        return Ok(());
    };
    if requestor != message.author {
        tracing::warn!(
            task_id = %task.effective_id(),
            author = %message.author,
            "ignoring task with mismatched requestor and envelope author"
        );
        return Ok(());
    }

    // Check keeper authorization for this inbound task using the same request gate as SSE mode.
    let auth_result = authorize_offer_request(&authority, &requestor, &task);

    if !auth_result.allowed {
        let person = PersonId::from_egregore(&requestor.0);
        publish_auth_denied_event(
            &egregore,
            &identity,
            &person,
            &request_skill(&task),
            AuthGate::Offer,
            &auth_result.reason,
        )
        .await;
        tracing::info!(
            author = %requestor,
            task_type = %task.effective_task_type(),
            reason = %auth_result.reason,
            "ignoring unauthorized message"
        );
        return Ok(());
    }

    if let Some(ref keeper_name) = auth_result.keeper {
        tracing::debug!(keeper = %keeper_name, "authorized as keeper");
    }
    metrics::record_auth_decision(AuthDecision::Allowed);

    tracing::debug!(hash = %task.hash, prompt_len = task.prompt.len(), "received task");
    let task_trace_id = task.context_trace_id();

    // Reject tasks without pre-planned tool calls
    if !task.is_direct() {
        tracing::warn!(
            task_hash = %task.hash,
            "rejecting task without tool_calls"
        );
        let failed = TaskFailed::new(
            task.effective_id().to_string(),
            identity.public_id(),
            TaskFailureReason::ExecutionError,
            Some("Servitor requires pre-planned tool_calls. Route through familiar for task decomposition.".into()),
        );
        egregore
            .publish_failed_with_trace(&failed, task_trace_id.as_deref(), None)
            .await?;
        return Ok(());
    }

    // Initialize MCP pool
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    // Initialize A2A pool
    let mut a2a_pool =
        A2aPool::from_config(config).map_err(|e| crate::error::ServitorError::Config {
            reason: format!("failed to create A2A pool: {}", e),
        })?;
    if !a2a_pool.is_empty() {
        a2a_pool
            .initialize_all()
            .await
            .map_err(|e| crate::error::ServitorError::Config {
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

    // Publish claim
    let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
    if let Err(e) = egregore.publish_claim(&claim).await {
        tracing::warn!(error = %e, "failed to publish claim");
        // Continue anyway — claim is advisory
    }

    // Execute pre-planned tool calls directly
    let result = crate::agent::direct::execute_direct(
        &task,
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
        Some(&egregore),
        Some(&authority),
        auth_result.keeper.as_deref(),
    )
    .await?;

    // Publish result
    egregore.publish_result(&result).await?;

    // Cleanup
    mcp_pool.shutdown_all().await?;

    tracing::info!(
        status = ?result.status,
        hash = %result.result_hash,
        "task complete"
    );

    Ok(())
}
