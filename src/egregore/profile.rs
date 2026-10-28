//! Profile building utilities for egregore network.

use chrono::Utc;
use std::collections::{BTreeMap, BTreeSet};

use crate::a2a::A2aPool;
use crate::agent::output_defense::defense_pipeline;
use crate::config::Config;
use crate::egregore::{
    DeploymentTargetSummary, EnvironmentSnapshot, ServitorManifest, ServitorProfile,
    SnapshotSensitivity, TargetSummary, ToolSummary, ToolsetSummary,
};
use crate::error::{LocalDiagnostic, ServitorError};
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

    // Tools whose names the operator approved for publication
    profile.tools = mcp_pool
        .public_tools()
        .into_iter()
        .map(|(name, _)| name.to_string())
        .collect();
    profile.tools.extend(
        a2a_pool
            .public_tools()
            .into_iter()
            .map(|(_, name, _)| name.to_string()),
    );
    profile.tools.sort();

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
    for (name, title) in mcp_pool.public_tools() {
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
            title,
        });
    }

    for toolset in toolsets_by_server.values_mut() {
        toolset
            .tools
            .sort_by(|left, right| left.name.cmp(&right.name));
    }

    for (agent_name, name, title) in a2a_pool.public_tools() {
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
            name: name.to_string(),
            title,
        });
    }

    for toolset in toolsets_by_server.values_mut() {
        toolset
            .tools
            .sort_by(|left, right| left.name.cmp(&right.name));
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
            // A probe record names a registered tool, says whether the probe
            // succeeded, and carries the tool's output (payload, through the
            // output-defense pipeline) or a bounded error code. The probe's
            // arguments and raw error text stay local.
            let outcome = if mcp_pool.registered_tool(&probe.name).is_none() {
                tracing::warn!(
                    target_id = %target.target_id,
                    tool = %LocalDiagnostic::new(&probe.name),
                    "snapshot probe names an unregistered tool"
                );
                status = "degraded".to_string();
                serde_json::json!({ "ok": false, "error_code": "unknown_tool" })
            } else {
                match mcp_pool
                    .call_tool(&probe.name, probe.arguments.clone())
                    .await
                {
                    Ok(result) => {
                        let (sanitized_output, _) =
                            defense_pipeline(&probe.name, &result.text_content());
                        if result.is_error {
                            status = "degraded".to_string();
                        } else if status == "configured" {
                            status = "probed".to_string();
                        }
                        serde_json::json!({
                            "tool": probe.name,
                            "ok": !result.is_error,
                            "output": sanitized_output,
                        })
                    }
                    Err(error) => {
                        tracing::warn!(
                            target_id = %target.target_id,
                            tool = %probe.name,
                            error = %LocalDiagnostic::new(&error.to_string()),
                            "snapshot probe failed"
                        );
                        status = "degraded".to_string();
                        serde_json::json!({
                            "tool": probe.name,
                            "ok": false,
                            "error_code": probe_error_code(&error),
                        })
                    }
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

/// Bounded code for a failed snapshot probe call.
fn probe_error_code(error: &ServitorError) -> &'static str {
    match error {
        ServitorError::McpValidation { .. } => "invalid_arguments",
        ServitorError::McpServerNotFound { .. } => "server_unavailable",
        ServitorError::Timeout { .. } => "timeout",
        _ => "call_failed",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::mcp::McpClient;
    use crate::test_support::{assert_same_bytes, ScriptedClient, SecretGen};

    /// One target probing `shell_execute` once per entry in `probes`.
    fn config_with_probes(probes: &[(&str, Value)]) -> Config {
        let mut config = Config::from_str("").unwrap();
        let calls = probes
            .iter()
            .map(|(name, arguments)| json!({ "name": name, "arguments": arguments }))
            .collect::<Vec<_>>();
        config.profile.targets = serde_json::from_value(json!([{
            "target_id": "db",
            "kind": "host",
            "summary": "database host",
            "snapshot_tool_calls": calls,
        }]))
        .unwrap();
        config
    }

    async fn probe_results(config: &Config, client: Box<dyn McpClient>) -> Value {
        let mut pool = McpPool::new();
        pool.register_test_tool("shell", "execute", client);
        let snapshots =
            build_environment_snapshots(&Identity::generate(), &pool, config, "manifest").await;
        assert_eq!(snapshots.len(), 1);
        snapshots[0].state["probe_results"].clone()
    }

    /// Advertised names and descriptions are published only when the
    /// operator approves them for that server.
    #[tokio::test]
    async fn unapproved_tools_stay_out_of_profile_and_manifest() {
        let tool = |name: &str| thallus_core::mcp::client::ToolDefinition {
            name: name.to_string(),
            description: Some("password=canary_7f3a".to_string()),
            input_schema: None,
        };
        let mut pool = McpPool::new();
        pool.register_server_tools("hidden", vec![tool("canary_7f3a")])
            .unwrap();
        let config = Config::from_str("").unwrap();
        let identity = Identity::generate();
        let a2a = A2aPool::new();
        let stats = RuntimeStats::default();

        let profile = build_profile(&identity, &pool, &a2a, &config, &stats, None).await;
        let manifest = build_manifest(&identity, &pool, &a2a, &config).await;

        for text in [
            serde_json::to_string(&profile).unwrap(),
            serde_json::to_string(&manifest).unwrap(),
        ] {
            assert!(!text.contains("canary_7f3a"), "{text}");
        }
    }

    #[tokio::test]
    async fn manifest_tool_titles_are_single_line_and_bounded() {
        let mut pool = McpPool::new();
        pool.approve_server("shell", crate::public::PublicApproval::all());
        pool.register_server_tools(
            "shell",
            vec![thallus_core::mcp::client::ToolDefinition {
                name: "execute".to_string(),
                description: Some(format!("Run a\ncommand\u{202E} {}", "x".repeat(400))),
                input_schema: None,
            }],
        )
        .unwrap();
        let config = Config::from_str("").unwrap();

        let manifest = build_manifest(&Identity::generate(), &pool, &A2aPool::new(), &config).await;

        let title = manifest.toolsets[0].tools[0].title.as_deref().unwrap();
        assert!(title.starts_with("Run a command x"), "{title}");
        assert!(
            !title.contains('\n') && !title.contains('\u{202E}'),
            "{title}"
        );
        assert_eq!(title.chars().count(), 200);
    }

    #[tokio::test]
    async fn probe_failures_publish_a_code_not_the_error_text() {
        let canary = SecretGen::CANARY;
        let config = config_with_probes(&[
            ("shell_execute", json!({ "command": "status" })),
            (&format!("shell_{canary}"), json!({})),
        ]);

        let results = probe_results(
            &config,
            ScriptedClient::call_error(format!("connect failed: password {canary}")),
        )
        .await;

        let text = results.to_string();
        assert!(!text.contains(canary), "{text}");
        assert_eq!(
            results,
            json!([
                { "tool": "shell_execute", "ok": false, "error_code": "call_failed" },
                { "ok": false, "error_code": "unknown_tool" },
            ])
        );
    }

    #[tokio::test]
    async fn probe_arguments_are_not_published() {
        let canary = SecretGen::CANARY;
        let config = config_with_probes(&[(
            "shell_execute",
            json!({ "command": format!("mysql -u monitor -p{canary} -e 'show status'") }),
        )]);

        let results = probe_results(&config, ScriptedClient::output("Uptime 42")).await;

        assert!(!results.to_string().contains(canary), "{results}");
        assert_eq!(
            results,
            json!([{ "tool": "shell_execute", "ok": true, "output": "Uptime 42" }])
        );
    }

    /// Information-flow property for the snapshot's probe record: varying
    /// a probe's arguments and its call error leaves the published probe
    /// results unchanged.
    #[tokio::test]
    async fn probe_results_ignore_arguments_and_error_text() {
        let mut reference: Option<Vec<u8>> = None;
        for seed in 0..32 {
            let mut gen = SecretGen::new(seed);
            let config = config_with_probes(&[("shell_execute", gen.arguments())]);
            let results = probe_results(&config, ScriptedClient::call_error(gen.secret())).await;
            let bytes = serde_json::to_vec(&results).unwrap();
            match &reference {
                None => reference = Some(bytes),
                Some(expected) => assert_same_bytes(expected, &bytes, &format!("seed {seed}")),
            }
        }
        let reference = String::from_utf8(reference.unwrap()).unwrap();
        assert!(!reference.contains(SecretGen::CANARY), "{reference}");
    }
}
