//! Profile building utilities for egregore network.

use chrono::Utc;
use std::collections::{BTreeMap, BTreeSet};

use crate::a2a::A2aPool;
use crate::config::Config;
use crate::egregore::{
    DeploymentTargetSummary, EnvironmentSnapshot, ScopeConstraints, ServitorManifest,
    ServitorProfile, SnapshotSensitivity, TargetSummary, ToolSummary, ToolsetSummary,
};
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
    manifest_ref: Option<&str>,
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

    profile.roles = config.profile.roles.clone();
    profile.labels = config.profile.labels.clone();
    profile.manifest_ref = manifest_ref.map(str::to_string);

    let mut kinds = BTreeSet::new();
    for target in &config.profile.targets {
        kinds.insert(target.kind.clone());
    }
    profile.target_summary = TargetSummary {
        count: config.profile.targets.len() as u64,
        kinds: kinds.into_iter().collect(),
    };

    profile
}

/// Build a planner-facing servitor manifest.
pub async fn build_manifest(
    identity: &Identity,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    config: &Config,
) -> ServitorManifest {
    let updated_at = Utc::now();
    let manifest_id = format!("manifest-{}", updated_at.to_rfc3339());
    let mut capabilities = mcp_pool.capabilities();
    capabilities.extend(a2a_pool.agents());
    capabilities.sort();
    capabilities.dedup();

    let mut toolsets_by_server: BTreeMap<String, ToolsetSummary> = BTreeMap::new();
    for (name, tool) in mcp_pool.all_tools() {
        let Some((server_name, _tool_name)) = mcp_pool.parse_tool_name(name) else {
            continue;
        };
        let transport = config
            .mcp
            .get(server_name)
            .map(|server| server.transport.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let entry = toolsets_by_server
            .entry(server_name.to_string())
            .or_insert_with(|| ToolsetSummary {
                server: server_name.to_string(),
                transport,
                tools: Vec::new(),
                resources: Vec::new(),
                resource_templates: Vec::new(),
            });
        entry.tools.push(ToolSummary {
            name: name.to_string(),
            title: tool.description.clone(),
        });
    }

    for toolset in toolsets_by_server.values_mut() {
        toolset.tools.sort_by(|left, right| left.name.cmp(&right.name));
    }

    for tool in a2a_pool.tools_for_llm() {
        let Some((agent_name, _)) = tool.name.split_once('_') else {
            continue;
        };
        let entry = toolsets_by_server
            .entry(agent_name.to_string())
            .or_insert_with(|| ToolsetSummary {
                server: agent_name.to_string(),
                transport: "a2a".to_string(),
                tools: Vec::new(),
                resources: Vec::new(),
                resource_templates: Vec::new(),
            });
        entry.tools.push(ToolSummary {
            name: tool.name,
            title: tool.description,
        });
    }

    for toolset in toolsets_by_server.values_mut() {
        toolset.tools.sort_by(|left, right| left.name.cmp(&right.name));
    }

    let deployment_targets = config
        .profile
        .targets
        .iter()
        .map(|target| DeploymentTargetSummary {
            target_id: target.target_id.clone(),
            kind: target.kind.clone(),
            summary: target.summary.clone(),
            roles: target.roles.clone(),
            snapshot_ref: None,
        })
        .collect();

    ServitorManifest {
        msg_type: "servitor_manifest".to_string(),
        servitor_id: identity.public_id(),
        manifest_id,
        profile_ref: None,
        roles: config.profile.roles.clone(),
        labels: config.profile.labels.clone(),
        capabilities,
        toolsets: toolsets_by_server.into_values().collect(),
        deployment_targets,
        policy_hints: None,
        updated_at,
    }
}

/// Build planner-facing environment snapshots for configured targets.
pub async fn build_environment_snapshots(
    identity: &Identity,
    mcp_pool: &McpPool,
    config: &Config,
    manifest_ref: &str,
) -> Vec<EnvironmentSnapshot> {
    let mut snapshots = Vec::new();

    for target in &config.profile.targets {
        let observed_at = Utc::now();
        let mut status = "configured".to_string();
        let mut probe_results = Vec::new();

        for probe in &target.snapshot_tool_calls {
            let outcome = match mcp_pool.call_tool(&probe.name, probe.arguments.clone()).await {
                Ok(result) => {
                    if result.is_error {
                        status = "degraded".to_string();
                    } else if status == "configured" {
                        status = "probed".to_string();
                    }
                    serde_json::json!({
                        "tool": probe.name,
                        "arguments": probe.arguments,
                        "ok": !result.is_error,
                        "output": result.text_content(),
                    })
                }
                Err(error) => {
                    status = "degraded".to_string();
                    serde_json::json!({
                        "tool": probe.name,
                        "arguments": probe.arguments,
                        "ok": false,
                        "error": error.to_string(),
                    })
                }
            };
            probe_results.push(outcome);
        }

        let summary = serde_json::json!({
            "status": status,
            "roles": target.roles.clone(),
            "probe_count": target.snapshot_tool_calls.len(),
        });
        let state = serde_json::json!({
            "target": {
                "target_id": target.target_id.clone(),
                "kind": target.kind.clone(),
                "summary": target.summary.clone(),
                "roles": target.roles.clone(),
            },
            "labels": config.profile.labels.clone(),
            "probe_results": probe_results,
        });

        snapshots.push(EnvironmentSnapshot {
            msg_type: "environment_snapshot".to_string(),
            snapshot_id: format!("snapshot-{}-{}", target.target_id, observed_at.to_rfc3339()),
            servitor_id: identity.public_id(),
            target_id: target.target_id.clone(),
            manifest_ref: manifest_ref.to_string(),
            kind: target.kind.clone(),
            summary,
            state,
            observed_at,
            ttl_secs: target.snapshot_ttl_secs,
            sensitivity: SnapshotSensitivity::Restricted,
        });
    }

    snapshots
}
