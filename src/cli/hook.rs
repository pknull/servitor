//! Hook mode implementation for egregore integration.

use std::path::PathBuf;

use crate::a2a::A2aPool;
use crate::agent::{create_provider, AgentExecutor};
use crate::authority::{load_runtime_authority, AuthRequest, PersonId};
use crate::config::Config;
use crate::egregore::{AuthGate, EgregoreClient, TaskClaim};
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::metrics::{self, AuthDecision};
use crate::runtime::publish_auth_denied_event;
use crate::scope::ScopeEnforcer;

/// Run servitor in hook mode (stdin JSON from egregore).
///
/// This mode reads a single task from stdin, executes it, and publishes
/// the result to the egregore network. Designed to be invoked by the
/// egregore daemon as a hook handler.
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

    // Check keeper authorization for this inbound task.
    let person = PersonId::from_egregore(&message.author.0);
    let auth_result = authority.authorize(&AuthRequest {
        person: person.clone(),
        skill: "*".to_string(), // Task intake doesn't specify skill yet
    });

    if !auth_result.allowed {
        publish_auth_denied_event(
            &egregore,
            &identity,
            &person,
            "*",
            AuthGate::Offer,
            &auth_result.reason,
        )
        .await;
        tracing::info!(
            author = %message.author.0,
            reason = %auth_result.reason,
            "ignoring unauthorized message"
        );
        return Ok(());
    }

    if let Some(ref keeper_name) = auth_result.keeper {
        tracing::debug!(keeper = %keeper_name, "authorized as keeper");
    }
    metrics::record_auth_decision(AuthDecision::Allowed);

    // Extract task from message
    let task = message
        .as_task()
        .ok_or_else(|| crate::ServitorError::Egregore {
            reason: "message is not a task".into(),
        })?;

    tracing::debug!(hash = %task.hash, prompt_len = task.prompt.len(), "received task");

    // Initialize components - hook mode requires LLM
    let llm_config = config.llm.as_ref().ok_or_else(|| ServitorError::Config {
        reason: "hook mode requires [llm] configuration for reasoning".into(),
    })?;
    let provider = create_provider(llm_config)?;
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

    // Publish claim
    let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
    if let Err(e) = egregore.publish_claim(&claim).await {
        tracing::warn!(error = %e, "failed to publish claim");
        // Continue anyway — claim is advisory
    }

    // Execute task with context fetching and authority
    let executor = AgentExecutor::new(
        provider.as_ref(),
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
    )
    .with_egregore(&egregore)
    .with_a2a_pool(&a2a_pool)
    .with_authority(&authority, auth_result.keeper.clone());

    let result = executor.execute(&task).await?;

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
