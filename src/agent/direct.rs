//! Direct tool call execution — bypasses LLM reasoning.
//!
//! When a task carries pre-planned tool calls (`task.tool_calls`),
//! servitor executes them sequentially against the MCP pool without
//! engaging an LLM provider. Authority and scope checks still apply.

use std::time::Instant;

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{
    Attestation, PlannedToolCall, Task, TaskResult, TaskStatus, TraceSpan, TraceSpanStatus,
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
/// Results are collected and returned as a signed `TaskResult`.
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
    let trace_enabled = config.publish_trace_spans && egregore.is_some();
    let trace_id = trace_enabled.then(new_uuid);
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
        // Validate scope and authority before execution
        validate_direct_call(
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

        // Record metrics
        let (provider_name, tool_name) = parse_prefixed_name(&planned.name);
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
                if output.is_error {
                    let error_msg = format!(
                        "tool '{}' returned error: {}",
                        planned.name, text
                    );
                    tracing::warn!(tool = %planned.name, "direct call returned error");
                    return build_error_result(
                        task,
                        identity,
                        &error_msg,
                        trace_id,
                        &trace_started_at,
                        &root_span_id,
                        egregore,
                        Some(execution_start.elapsed().as_secs()),
                    )
                    .await;
                }
                call_results.push(DirectCallResult {
                    tool_name: planned.name.clone(),
                    output: text,
                });
            }
            Err(e) => {
                let error_msg = format!("tool '{}' failed: {}", planned.name, e);
                tracing::error!(tool = %planned.name, error = %e, "direct call failed");
                return build_error_result(
                    task,
                    identity,
                    &error_msg,
                    trace_id,
                    &trace_started_at,
                    &root_span_id,
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
    let task_result = build_signed_result(task, identity, TaskStatus::Success, Some(result_value), None, trace_id.clone(), Some(elapsed))?;

    // Publish root trace span
    if let (Some(trace_id), Some(root_span_id), Some(started_at)) =
        (&trace_id, &root_span_id, trace_started_at)
    {
        publish_root_span(egregore, task, trace_id, root_span_id, started_at, &task_result).await;
    }

    Ok(task_result)
}

/// Validate a single direct tool call against scope and authority policies.
fn validate_direct_call(
    task: &Task,
    planned: &PlannedToolCall,
    mcp_pool: &McpPool,
    scope_enforcer: &ScopeEnforcer,
    authority: Option<&Authority>,
    keeper_name: Option<&str>,
) -> Result<()> {
    // Verify the tool exists in the MCP pool
    if mcp_pool.parse_tool_name(&planned.name).is_none() {
        return Err(ServitorError::PlanValidation {
            reason: format!("unknown tool in direct call: {}", planned.name),
        });
    }

    let (provider_name, tool_name) = parse_prefixed_name(&planned.name);

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
                reason: format!(
                    "direct task '{}' scope violation: {}",
                    task.hash, reason
                ),
            },
            other => other,
        })?;

    Ok(())
}

fn build_signed_result(
    task: &Task,
    identity: &Identity,
    status: TaskStatus,
    result: Option<serde_json::Value>,
    error: Option<String>,
    trace_id: Option<String>,
    duration_seconds: Option<u64>,
) -> Result<TaskResult> {
    let result_hash = compute_result_hash(&result, &error, trace_id.as_deref());
    let signature = identity.sign_hash(&result_hash);

    Ok(TaskResult {
        msg_type: "task_result".to_string(),
        task_id: task.effective_id().to_string(),
        servitor: identity.public_id(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        task_hash: task.hash.clone(),
        result_hash,
        status,
        result,
        error,
        duration_seconds,
        plan_hash: None,
        attestation: Attestation {
            servitor_id: identity.public_id(),
            signature,
            timestamp: Utc::now(),
        },
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
    egregore: Option<&EgregoreClient>,
    duration_seconds: Option<u64>,
) -> Result<TaskResult> {
    let result = build_signed_result(
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
        publish_root_span(egregore, task, trace_id, root_span_id, *started_at, &result).await;
    }

    Ok(result)
}

fn compute_result_hash(
    result: &Option<serde_json::Value>,
    error: &Option<String>,
    trace_id: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    if let Some(r) = result {
        hasher.update(serde_json::to_string(r).unwrap_or_default().as_bytes());
    }
    if let Some(e) = error {
        hasher.update(e.as_bytes());
    }
    if let Some(tid) = trace_id {
        hasher.update(tid.as_bytes());
    }
    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Best-effort split of a prefixed tool name (e.g. "shell_execute" → ("shell", "execute")).
/// For flat names without a separator, returns (name, name) so authority constructs
/// a matchable "name:name" pattern. Used for metrics (approximate); authority checks
/// in `validate_direct_call` use the MCP pool's own knowledge for accurate resolution.
fn parse_prefixed_name(prefixed: &str) -> (&str, &str) {
    prefixed.split_once('_').unwrap_or((prefixed, prefixed))
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
        None,
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

    #[test]
    fn parse_prefixed_name_splits_correctly() {
        let (provider, tool) = parse_prefixed_name("shell_execute");
        assert_eq!(provider, "shell");
        assert_eq!(tool, "execute");
    }

    #[test]
    fn parse_prefixed_name_handles_no_separator() {
        let (provider, tool) = parse_prefixed_name("noprefix");
        assert_eq!(provider, "noprefix");
        assert_eq!(tool, "noprefix");
    }

    #[test]
    fn result_hash_is_deterministic() {
        let r = Some(serde_json::json!({"key": "value"}));
        let h1 = compute_result_hash(&r, &None, None);
        let h2 = compute_result_hash(&r, &None, None);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn result_hash_changes_with_error() {
        let r = Some(serde_json::json!({"key": "value"}));
        let h1 = compute_result_hash(&r, &None, None);
        let h2 = compute_result_hash(&r, &Some("oops".to_string()), None);
        assert_ne!(h1, h2);
    }
}
