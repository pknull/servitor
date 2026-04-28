//! Direct tool call execution — bypasses LLM reasoning.
//!
//! When a task carries pre-planned tool calls (`task.tool_calls`),
//! servitor executes them sequentially against the MCP pool without
//! engaging an LLM provider. Authority and scope checks still apply.

use std::time::Instant;

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{
    PlannedToolCall, Task, TaskResult, TaskStatus, TraceSpan, TraceSpanStatus,
};
use crate::egregore::EgregoreClient;
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::{McpPool, ToolCallResult};
use crate::metrics::{self, Timer, ToolCallStatus};
use crate::scope::ScopeEnforcer;

/// Result of a single successful direct tool call.
#[derive(Debug, Clone)]
struct DirectCallResult {
    tool_name: String,
    output: String,
}

/// Execute pre-planned tool calls directly without LLM involvement.
///
/// Each tool call is validated (scope + authority), then executed
/// sequentially. If any call fails, the entire task fails immediately.
/// Results are collected and returned as a `TaskResult`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_direct(
    task: &Task,
    mcp_pool: &McpPool,
    scope_enforcer: &ScopeEnforcer,
    identity: &Identity,
    config: &AgentConfig,
    egregore: Option<&EgregoreClient>,
    authority: Option<&Authority>,
    keeper_name: Option<&str>,
) -> Result<TaskResult> {
    let execution_start = Instant::now();
    let inherited_trace_id = task_context_string(task, "trace_id");
    let parent_span_id = task
        .context_span_id()
        .or_else(|| task.context_parent_span_id())
        .or_else(|| task_context_string(task, "parent_span_id"));
    let trace_enabled = config.publish_trace_spans && egregore.is_some();
    let trace_id = if trace_enabled {
        Some(inherited_trace_id.clone().unwrap_or_else(new_uuid))
    } else {
        inherited_trace_id
    };
    let root_span_id = trace_enabled.then(new_uuid);
    let trace_started_at = trace_enabled.then(Utc::now);

    tracing::info!(
        task_hash = %task.hash,
        tool_count = task.tool_calls.len(),
        "direct execution: {} tool call(s)",
        task.tool_calls.len()
    );

    let mut call_results: Vec<DirectCallResult> = Vec::with_capacity(task.tool_calls.len());

    for (idx, planned) in task.tool_calls.iter().enumerate() {
        // Validate scope and authority before execution. Returns the authoritative
        // (server, tool) split from the MCP pool — reused below for metrics, trace
        // tags, and the authority-skill string so every downstream observation
        // matches the registered tool boundary even when tool names contain `_`.
        let (provider_name, tool_name) = validate_direct_call(
            task,
            planned,
            mcp_pool,
            scope_enforcer,
            authority,
            keeper_name,
        )?;

        tracing::debug!(
            idx = idx,
            tool = %planned.name,
            "direct: executing tool call"
        );

        let timer = Timer::start();
        let tool_start = trace_enabled.then(Utc::now);
        let result = mcp_pool
            .call_tool(&planned.name, planned.arguments.clone())
            .await;
        let duration = timer.elapsed_secs();
        let status = match &result {
            Ok(output) if !output.is_error => ToolCallStatus::Success,
            _ => ToolCallStatus::Error,
        };
        metrics::record_tool_call(tool_name, provider_name, status);
        metrics::record_tool_call_duration(tool_name, duration);

        // Publish trace span for this tool call
        if let (Some(ref trace_id), Some(ref root_span_id), Some(tool_start)) =
            (&trace_id, &root_span_id, tool_start)
        {
            publish_tool_span(
                egregore,
                trace_id,
                root_span_id,
                provider_name,
                tool_name,
                tool_start,
                &result,
            )
            .await;
        }

        match result {
            Ok(output) => {
                let text = output.text_content();
                let (sanitized_text, _) =
                    crate::agent::output_defense::defense_pipeline(&planned.name, &text);
                if output.is_error {
                    let error_msg =
                        format!("tool '{}' returned error: {}", planned.name, sanitized_text);
                    tracing::warn!(tool = %planned.name, "direct call returned error");
                    return build_error_result(
                        task,
                        identity,
                        &error_msg,
                        trace_id,
                        &trace_started_at,
                        &root_span_id,
                        parent_span_id.as_deref(),
                        egregore,
                        Some(execution_start.elapsed().as_secs()),
                    )
                    .await;
                }
                call_results.push(DirectCallResult {
                    tool_name: planned.name.clone(),
                    output: sanitized_text,
                });
            }
            Err(e) => {
                let sanitized_error = crate::agent::sanitize::sanitize_tool_result(&e.to_string());
                let error_msg = format!("tool '{}' failed: {}", planned.name, sanitized_error);
                tracing::error!(tool = %planned.name, error = %e, "direct call failed");
                return build_error_result(
                    task,
                    identity,
                    &error_msg,
                    trace_id,
                    &trace_started_at,
                    &root_span_id,
                    parent_span_id.as_deref(),
                    egregore,
                    Some(execution_start.elapsed().as_secs()),
                )
                .await;
            }
        }
    }

    // Build success result with all outputs
    let result_value = serde_json::json!({
        "mode": "direct",
        "tool_results": call_results.iter().map(|r| {
            serde_json::json!({
                "tool": r.tool_name,
                "output": r.output,
            })
        }).collect::<Vec<_>>(),
    });

    let elapsed = execution_start.elapsed().as_secs();
    let task_result = build_result(
        task,
        identity,
        TaskStatus::Success,
        Some(result_value),
        None,
        trace_id.clone(),
        Some(elapsed),
    )?;

    // Publish root trace span
    if let (Some(trace_id), Some(root_span_id), Some(started_at)) =
        (&trace_id, &root_span_id, trace_started_at)
    {
        publish_root_span(
            egregore,
            task,
            trace_id,
            root_span_id,
            started_at,
            &task_result,
            parent_span_id.as_deref(),
        )
        .await;
    }

    Ok(task_result)
}

/// Validate a single direct tool call against scope and authority policies.
/// Returns the authoritative (server_name, tool_name) split, resolved against
/// the MCP pool's actual registration — so policy decisions and observability
/// downstream key off the registered tool boundary, not a heuristic split.
fn validate_direct_call<'a>(
    task: &Task,
    planned: &'a PlannedToolCall,
    mcp_pool: &'a McpPool,
    scope_enforcer: &ScopeEnforcer,
    authority: Option<&Authority>,
    keeper_name: Option<&str>,
) -> Result<(&'a str, &'a str)> {
    // Authoritative parse against the registered tool. If this returns None the
    // tool isn't in the pool — surface as a plan-validation error rather than
    // falling back to a heuristic split, which would let policy keys drift.
    let (provider_name, tool_name) =
        mcp_pool
            .parse_tool_name(&planned.name)
            .ok_or_else(|| ServitorError::PlanValidation {
                reason: format!("unknown tool in direct call: {}", planned.name),
            })?;

    // Check authority (keeper skill permissions)
    if let (Some(authority), Some(keeper)) = (authority, keeper_name) {
        let skill = format!("{}:{}", provider_name, tool_name);
        let auth_result = authority.authorize_skill(keeper, &skill);
        if !auth_result.allowed {
            return Err(ServitorError::Unauthorized {
                reason: format!(
                    "keeper '{}' not authorized for skill '{}': {}",
                    keeper, skill, auth_result.reason
                ),
            });
        }
    }

    // Check scope policy
    scope_enforcer
        .check(
            provider_name,
            tool_name,
            &planned.arguments,
            task.scope_override.as_ref(),
        )
        .map_err(|error| match error {
            ServitorError::ScopeViolation { reason } => ServitorError::ScopeViolation {
                reason: format!("direct task '{}' scope violation: {}", task.hash, reason),
            },
            other => other,
        })?;

    Ok((provider_name, tool_name))
}

fn build_result(
    task: &Task,
    identity: &Identity,
    status: TaskStatus,
    result: Option<serde_json::Value>,
    error: Option<String>,
    trace_id: Option<String>,
    duration_seconds: Option<u64>,
) -> Result<TaskResult> {
    let correlation_id = uuid::Uuid::new_v4().to_string();
    let servitor = identity.public_id();
    let task_id = task.effective_id().to_string();
    let result_hash = compute_result_hash(
        &task_id,
        &servitor,
        &correlation_id,
        &task.hash,
        &status,
        &result,
        &error,
        duration_seconds,
        &trace_id,
    );
    Ok(TaskResult {
        msg_type: "task_result".to_string(),
        task_id,
        servitor: servitor.clone(),
        correlation_id,
        task_hash: task.hash.clone(),
        result_hash,
        status,
        result,
        error,
        duration_seconds,
        trace_id,
    })
}

#[allow(clippy::too_many_arguments)]
async fn build_error_result(
    task: &Task,
    identity: &Identity,
    error_msg: &str,
    trace_id: Option<String>,
    trace_started_at: &Option<chrono::DateTime<Utc>>,
    root_span_id: &Option<String>,
    parent_span_id: Option<&str>,
    egregore: Option<&EgregoreClient>,
    duration_seconds: Option<u64>,
) -> Result<TaskResult> {
    let result = build_result(
        task,
        identity,
        TaskStatus::Error,
        None,
        Some(error_msg.to_string()),
        trace_id.clone(),
        duration_seconds,
    )?;

    if let (Some(trace_id), Some(root_span_id), Some(started_at)) =
        (&trace_id, root_span_id, trace_started_at)
    {
        publish_root_span(
            egregore,
            task,
            trace_id,
            root_span_id,
            *started_at,
            &result,
            parent_span_id,
        )
        .await;
    }

    Ok(result)
}

fn compute_result_hash(
    task_id: &str,
    servitor: &crate::identity::PublicId,
    correlation_id: &str,
    task_hash: &str,
    status: &TaskStatus,
    result: &Option<serde_json::Value>,
    error: &Option<String>,
    duration_seconds: Option<u64>,
    trace_id: &Option<String>,
) -> String {
    // Hash payload as a Serialize-derived struct, NOT a serde_json::json!({...})
    // literal. With serde_json's default `Map` (BTreeMap, alphabetic), json!()
    // would serialize keys in sorted order; the derived struct emits fields in
    // declaration order. The byte-order difference would silently change
    // result_hash output for any downstream consumer that compared hashes
    // across versions.
    #[derive(Serialize)]
    struct ResultHashPayload<'a> {
        task_id: &'a str,
        servitor: &'a crate::identity::PublicId,
        correlation_id: &'a str,
        task_hash: &'a str,
        status: &'a TaskStatus,
        result: &'a Option<serde_json::Value>,
        error: &'a Option<String>,
        duration_seconds: Option<u64>,
        trace_id: &'a Option<String>,
    }

    let payload = ResultHashPayload {
        task_id,
        servitor,
        correlation_id,
        task_hash,
        status,
        result,
        error,
        duration_seconds,
        trace_id,
    };

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload).unwrap_or_default());
    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn task_context_string(task: &Task, key: &str) -> Option<String> {
    task.context
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn publish_tool_span(
    egregore: Option<&EgregoreClient>,
    trace_id: &str,
    parent_span_id: &str,
    provider_name: &str,
    tool_name: &str,
    start_ts: chrono::DateTime<Utc>,
    result: &Result<ToolCallResult>,
) {
    let Some(egregore) = egregore else { return };

    let status = match result {
        Ok(output) if !output.is_error => TraceSpanStatus::Ok,
        _ => TraceSpanStatus::Error,
    };

    let span = TraceSpan::new(
        trace_id,
        new_uuid(),
        Some(parent_span_id.to_string()),
        format!("direct:{}:{}", provider_name, tool_name),
        "servitor",
        start_ts,
        Utc::now(),
        status,
    );

    if let Err(e) = egregore.publish_trace_span(&span).await {
        tracing::debug!(error = %e, "failed to publish direct tool trace span");
    }
}

async fn publish_root_span(
    egregore: Option<&EgregoreClient>,
    task: &Task,
    trace_id: &str,
    root_span_id: &str,
    start_ts: chrono::DateTime<Utc>,
    result: &TaskResult,
    parent_span_id: Option<&str>,
) {
    let Some(egregore) = egregore else { return };

    let status = match result.status {
        TaskStatus::Success => TraceSpanStatus::Ok,
        TaskStatus::Error => TraceSpanStatus::Error,
        TaskStatus::Timeout => TraceSpanStatus::Timeout,
    };

    let mut span = TraceSpan::new(
        trace_id,
        root_span_id,
        parent_span_id.map(str::to_string),
        "direct_execution",
        "servitor",
        start_ts,
        Utc::now(),
        status,
    );
    span.attributes.insert(
        "task_hash".to_string(),
        serde_json::Value::String(task.hash.clone()),
    );
    span.attributes.insert(
        "mode".to_string(),
        serde_json::Value::String("direct".to_string()),
    );
    span.attributes.insert(
        "tool_count".to_string(),
        serde_json::json!(task.tool_calls.len()),
    );

    if let Err(e) = egregore.publish_trace_span(&span).await {
        tracing::debug!(error = %e, "failed to publish direct root trace span");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn result_hash_is_deterministic() {
        let r = Some(serde_json::json!({"key": "value"}));
        let servitor = Identity::generate().public_id();
        let status = TaskStatus::Success;
        let trace_id = Some("trace-1".to_string());
        let h1 = compute_result_hash(
            "task-1",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &None,
            Some(5),
            &trace_id,
        );
        let h2 = compute_result_hash(
            "task-1",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &None,
            Some(5),
            &trace_id,
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn result_hash_changes_with_error() {
        let r = Some(serde_json::json!({"key": "value"}));
        let servitor = Identity::generate().public_id();
        let status = TaskStatus::Success;
        let trace_id = None;
        let h1 = compute_result_hash(
            "task-1",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &None,
            Some(5),
            &trace_id,
        );
        let h2 = compute_result_hash(
            "task-1",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &Some("oops".to_string()),
            Some(5),
            &trace_id,
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn result_hash_changes_with_task_metadata() {
        let r = Some(serde_json::json!({"key": "value"}));
        let servitor = Identity::generate().public_id();
        let status = TaskStatus::Success;
        let trace_id = None;
        let h1 = compute_result_hash(
            "task-1",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &None,
            Some(5),
            &trace_id,
        );
        let h2 = compute_result_hash(
            "task-2",
            &servitor,
            "corr-1",
            "task-hash-1",
            &status,
            &r,
            &None,
            Some(5),
            &trace_id,
        );
        assert_ne!(h1, h2);
    }

    /// Locks the byte-format of the hashed payload. If a future refactor
    /// changes field order, switches to serde_json::json!({...}), or otherwise
    /// alters the JSON shape, this test fails — preventing silent
    /// hash-format regressions for any downstream consumer comparing
    /// result_hashes across upgrades.
    #[test]
    fn result_hash_format_locked() {
        use crate::identity::PublicId;

        // Use a fixed PublicId rather than Identity::generate() so the input is
        // fully deterministic. Format must satisfy PublicId::is_valid_format
        // (53 chars: '@' + 44 base64 chars + '.ed25519').
        let servitor = PublicId(format!("@{}.ed25519", "A".repeat(43) + "="));
        let result = Some(serde_json::json!({"answer": 42}));
        let trace_id = Some("trace-fixture".to_string());

        let hash = compute_result_hash(
            "task-fixture",
            &servitor,
            "corr-fixture",
            "task-hash-fixture",
            &TaskStatus::Success,
            &result,
            &None,
            Some(7),
            &trace_id,
        );

        // Locked output. Update only when intentionally changing the
        // hash payload format (which is a downstream-visible breaking change).
        assert_eq!(
            hash,
            "5d46352b090e62dedd916b90ff5f8b2cdcae9758938d84d460891b2084e615b8",
            "result_hash byte-format changed unexpectedly"
        );
    }
}
