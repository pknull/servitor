//! Profile building utilities for egregore network.

use crate::a2a::A2aPool;
use crate::config::Config;
use crate::egregore::{ScopeConstraints, ServitorProfile};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::runtime::RuntimeStats;

/// Build a ServitorProfile for publishing.
pub async fn build_profile(
    identity: &Identity,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    config: &Config,
    runtime_stats: &RuntimeStats,
) -> ServitorProfile {
    let mut profile =
        ServitorProfile::new(identity.public_id(), config.heartbeat.interval_secs * 1000);

    profile.version = env!("CARGO_PKG_VERSION").to_string();
    if config.heartbeat.include_runtime_monitoring {
        profile.uptime_secs = runtime_stats.uptime_secs();
        profile.mcp_servers = mcp_pool.server_statuses().await;
        profile.load = runtime_stats.load();
        profile.stats = runtime_stats.stats();
        profile.last_task_ts = runtime_stats.last_task_ts;
    }

    // Add capabilities from MCP servers and A2A agents
    profile.capabilities = mcp_pool.capabilities();
    profile.capabilities.extend(a2a_pool.agents());

    // Add tools from MCP pool
    profile.tools = mcp_pool
        .all_tools()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    // Add tools from A2A pool
    for tool in a2a_pool.tools_for_llm() {
        profile.tools.push(tool.name);
    }

    // Add MCP scopes
    for (name, mcp_config) in &config.mcp {
        profile.scopes.insert(
            name.clone(),
            ScopeConstraints {
                allow: mcp_config.scope.allow.clone(),
                block: mcp_config.scope.block.clone(),
            },
        );
    }

    // Add A2A scopes
    for (name, a2a_config) in &config.a2a {
        profile.scopes.insert(
            name.clone(),
            ScopeConstraints {
                allow: a2a_config.scope.allow.clone(),
                block: a2a_config.scope.block.clone(),
            },
        );
    }

    profile
}
