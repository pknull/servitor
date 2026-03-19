//! Direct task execution command.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::a2a::A2aPool;
use crate::agent::{create_provider, AgentExecutor};
use crate::authority::{authorize_local_exec, load_runtime_authority};
use crate::config::Config;
use crate::egregore::EgregoreClient;
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Execute a task directly (for testing/development).
///
/// This bypasses the egregore network and executes a task immediately
/// using the local MCP server pool and LLM provider.
pub async fn run_exec(
    config: &Config,
    prompt: &str,
    insecure: bool,
    dry_run: bool,
    plan_first: bool,
) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;
    let authority = load_runtime_authority(&identity_dir, insecure)?;
    let keeper_name = authorize_local_exec(&authority, &identity)?;
    let egregore = EgregoreClient::new(&config.egregore.api_url);

    tracing::info!(id = %identity.public_id(), "executing task");

    // Initialize components - exec mode requires LLM
    let llm_config = config.llm.as_ref().ok_or_else(|| ServitorError::Config {
        reason: "exec mode requires [llm] configuration for reasoning".into(),
    })?;
    let provider = create_provider(llm_config)?;
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    // Initialize A2A pool
    let mut a2a_pool = A2aPool::from_config(config).map_err(|e| ServitorError::Config {
        reason: format!("failed to create A2A pool: {}", e),
    })?;
    if !a2a_pool.is_empty() {
        a2a_pool.initialize_all().await.map_err(|e| ServitorError::Config {
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

    // Build a task
    let task = crate::egregore::Task {
        msg_type: "task".to_string(),
        id: None,
        hash: format!("{:x}", md5_hash(prompt)),
        task_type: None,
        request: Some(prompt.to_string()),
        requestor: None,
        prompt: prompt.to_string(),
        required_caps: vec![],
        parent_id: None,
        context: std::collections::HashMap::new(),
        scope_override: None,
        priority: 0,
        timeout_secs: Some(config.agent.timeout_secs),
        author: None,
        keeper: None,
    };

    let executor = AgentExecutor::new(
        provider.as_ref(),
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
    )
    .with_egregore(&egregore)
    .with_a2a_pool(&a2a_pool)
    .with_authority(&authority, keeper_name);

    let mut published_plan_hash = None;

    if dry_run || plan_first {
        let plan = executor.plan(&task).await?;
        println!(
            "Plan: {}",
            serde_json::to_string_pretty(&plan).unwrap_or_default()
        );

        if should_publish_plan(plan_first) {
            let published_hash = egregore.publish_plan(&plan).await?;
            println!("Plan published: {}", published_hash);
            published_plan_hash = Some(plan.plan_hash.clone());
        }

        if dry_run {
            mcp_pool.shutdown_all().await?;
            return Ok(());
        }
    }

    let result = executor
        .execute_with_plan_hash(&task, published_plan_hash)
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
    if let Some(ref plan_hash) = result.plan_hash {
        println!("Plan hash: {}", plan_hash);
    }

    // Cleanup
    mcp_pool.shutdown_all().await?;

    Ok(())
}

/// Check if we should publish the plan to egregore.
fn should_publish_plan(plan_first: bool) -> bool {
    plan_first
}

/// Simple hash for task ID generation in exec mode.
fn md5_hash(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_keeps_plan_local() {
        assert!(!should_publish_plan(false));
    }

    #[test]
    fn plan_first_publishes_plan() {
        assert!(should_publish_plan(true));
    }
}
