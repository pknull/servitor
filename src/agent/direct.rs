//! Direct tool call execution — bypasses LLM reasoning.
//!
//! When a task carries pre-planned tool calls (`task.tool_calls`),
//! servitor executes them sequentially against the MCP pool without
//! engaging an LLM provider. Authority and scope checks still apply.

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{PlannedToolCall, Task, TaskResult, TaskStatus};
use crate::egregore::telemetry::{
    self, ExecutionSpanRecord, TelemetryId, ToolOutcome, ToolSpanRecord, TraceSpan, TraceSpanStatus,
};
use crate::egregore::EgregoreClient;
use crate::error::{CallRejection, LocalDiagnostic, Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::{McpPool, RegisteredToolId};
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
    let trace = egregore
        .filter(|_| config.publish_trace_spans)
        .map(|egregore| ExecutionTrace::start(task, egregore));
    let trace_id = match &trace {
        Some(trace) => Some(trace.trace_id.clone()),
        None => task.inherited_trace_id(),
    }
    .map(|id| id.as_str().to_string());

    tracing::info!(
        task_hash = %task.hash,
        tool_count = task.tool_calls.len(),
        "direct execution: {} tool call(s)",
        task.tool_calls.len()
    );

    let mut call_results: Vec<DirectCallResult> = Vec::with_capacity(task.tool_calls.len());

    for (idx, planned) in task.tool_calls.iter().enumerate() {
        // Validate scope and authority before execution. Returns the tool's
        // registered identity from the MCP pool — reused below for metrics,
        // trace spans, and the authority-skill string so every downstream
        // observation matches the registered tool boundary even when tool
        // names contain `_`.
        let tool = validate_direct_call(
            idx,
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
        let tool_start = telemetry::now();
        let result = mcp_pool
            .call_tool(&planned.name, planned.arguments.clone())
            .await;
        let duration = timer.elapsed_secs();
        let outcome = match &result {
            Ok(output) if !output.is_error => ToolOutcome::Ok,
            Ok(_) => ToolOutcome::ToolError,
            Err(_) => ToolOutcome::CallError,
        };
        let status = match outcome {
            ToolOutcome::Ok => ToolCallStatus::Success,
            ToolOutcome::ToolError | ToolOutcome::CallError => ToolCallStatus::Error,
        };
        metrics::record_tool_call(tool.tool(), tool.provider(), status);
        metrics::record_tool_call_duration(tool.tool(), duration);

        if let Some(trace) = &trace {
            trace
                .publish_tool(tool.clone(), idx, tool_start, outcome)
                .await;
        }

        match result {
            Ok(output) => {
                let text = output.text_content();
                let (sanitized_text, _) =
                    crate::agent::output_defense::defense_pipeline(&planned.name, &text);
                if output.is_error {
                    let error_msg = format!(
                        "tool '{}' returned error: {}",
                        tool.public_prefixed(),
                        sanitized_text
                    );
                    tracing::warn!(tool = %planned.name, "direct call returned error");
                    return build_error_result(
                        task,
                        identity,
                        &error_msg,
                        trace_id,
                        trace.as_ref(),
                        Some(execution_start.elapsed().as_secs()),
                    )
                    .await;
                }
                call_results.push(DirectCallResult {
                    tool_name: tool.public_prefixed(),
                    output: sanitized_text,
                });
            }
            Err(e) => {
                // Error text comes from the transport, the server or the
                // schema check and can quote URLs or argument values; the
                // requester gets a code and the text stays in the local log.
                let error_msg = format!(
                    "tool '{}' failed: {}",
                    tool.public_prefixed(),
                    call_failure_code(&e)
                );
                tracing::error!(tool = %planned.name, error = %e, "direct call failed");
                return build_error_result(
                    task,
                    identity,
                    &error_msg,
                    trace_id,
                    trace.as_ref(),
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
        trace_id,
        Some(elapsed),
    )?;

    if let Some(trace) = &trace {
        trace.publish_execution(TraceSpanStatus::Ok).await;
    }

    Ok(task_result)
}

/// Trace state for one execution whose spans are published.
///
/// Holds only public metadata: identifiers that are generated or inherited
/// from the task's trace context, the task's call count, and the start time.
struct ExecutionTrace<'a> {
    egregore: &'a EgregoreClient,
    trace_id: TelemetryId,
    span_id: TelemetryId,
    parent_span_id: Option<TelemetryId>,
    execution_id: TelemetryId,
    tool_count: usize,
    start: DateTime<Utc>,
}

impl<'a> ExecutionTrace<'a> {
    /// Join the task's trace when its context names one; otherwise start a
    /// new trace.
    fn start(task: &Task, egregore: &'a EgregoreClient) -> Self {
        Self {
            egregore,
            trace_id: task
                .inherited_trace_id()
                .unwrap_or_else(TelemetryId::generate),
            span_id: TelemetryId::generate(),
            parent_span_id: task.inherited_span_id(),
            execution_id: TelemetryId::generate(),
            tool_count: task.tool_calls.len(),
            start: telemetry::now(),
        }
    }

    async fn publish_tool(
        &self,
        tool: RegisteredToolId,
        call_index: usize,
        start: DateTime<Utc>,
        outcome: ToolOutcome,
    ) {
        let span = TraceSpan::tool(ToolSpanRecord {
            trace_id: self.trace_id.clone(),
            span_id: TelemetryId::generate(),
            parent_span_id: self.span_id.clone(),
            execution_id: self.execution_id.clone(),
            tool,
            call_index,
            start,
            end: telemetry::now(),
            outcome,
        });
        if let Err(e) = self.egregore.publish_trace_span(&span).await {
            tracing::debug!(error = %e, "failed to publish direct tool trace span");
        }
    }

    async fn publish_execution(&self, status: TraceSpanStatus) {
        let span = TraceSpan::execution(ExecutionSpanRecord {
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
            parent_span_id: self.parent_span_id.clone(),
            execution_id: self.execution_id.clone(),
            tool_count: self.tool_count,
            start: self.start,
            end: telemetry::now(),
            status,
        });
        if let Err(e) = self.egregore.publish_trace_span(&span).await {
            tracing::debug!(error = %e, "failed to publish direct execution trace span");
        }
    }
}

/// Validate a single direct tool call against scope and authority policies.
/// Returns the tool's registered identity from the MCP pool, so policy
/// decisions and observability downstream key off the registered tool
/// boundary, not a heuristic split.
///
/// A refusal is a [`ServitorError::CallRejected`]: a closed
/// [`CallRejection`] for the requester and a sanitized diagnostic for the
/// local log.
fn validate_direct_call(
    call_index: usize,
    task: &Task,
    planned: &PlannedToolCall,
    mcp_pool: &McpPool,
    scope_enforcer: &ScopeEnforcer,
    authority: Option<&Authority>,
    keeper_name: Option<&str>,
) -> Result<RegisteredToolId> {
    let rejected = |rejection, diagnostic: LocalDiagnostic| ServitorError::CallRejected {
        call_index,
        rejection,
        diagnostic,
    };

    // Authoritative lookup against the registered tool. An unknown tool is a
    // plan-validation error rather than a heuristic split, which would let
    // policy keys drift.
    let Some(tool) = mcp_pool.registered_tool(&planned.name) else {
        return Err(rejected(
            CallRejection::UnknownTool,
            LocalDiagnostic::new(&format!("unknown tool in direct call: {}", planned.name)),
        ));
    };

    // Check authority (keeper skill permissions)
    if let (Some(authority), Some(keeper)) = (authority, keeper_name) {
        let skill = tool.to_string();
        let auth_result = authority.authorize_skill(keeper, &skill);
        if !auth_result.allowed {
            let diagnostic = LocalDiagnostic::new(&format!(
                "keeper '{}' not authorized for skill '{}': {}",
                keeper, skill, auth_result.reason
            ));
            return Err(rejected(CallRejection::Unauthorized { tool }, diagnostic));
        }
    }

    // Check scope policy
    match scope_enforcer.check(
        tool.provider(),
        tool.tool(),
        &planned.arguments,
        task.scope_override.as_ref(),
    ) {
        Ok(()) => Ok(tool),
        Err(ServitorError::ScopeViolation { rule, diagnostic }) => {
            Err(rejected(CallRejection::Scope { tool, rule }, diagnostic))
        }
        Err(other) => Err(other),
    }
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

/// The bounded reason a failed tool call publishes in `task_result.error`.
fn call_failure_code(error: &ServitorError) -> &'static str {
    match error {
        ServitorError::McpValidation { .. } => {
            "invalid_arguments (the arguments do not match the tool's input schema)"
        }
        ServitorError::McpServerNotFound { .. } => "server_unavailable",
        ServitorError::Http(_) => "transport_error",
        ServitorError::Json(_) => "decode_error",
        ServitorError::Mcp { .. } => "mcp_error",
        _ => "call_failed",
    }
}

async fn build_error_result(
    task: &Task,
    identity: &Identity,
    error_msg: &str,
    trace_id: Option<String>,
    trace: Option<&ExecutionTrace<'_>>,
    duration_seconds: Option<u64>,
) -> Result<TaskResult> {
    let result = build_result(
        task,
        identity,
        TaskStatus::Error,
        None,
        Some(error_msg.to_string()),
        trace_id,
        duration_seconds,
    )?;

    if let Some(trace) = trace {
        trace.publish_execution(TraceSpanStatus::Error).await;
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
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
            hash, "5d46352b090e62dedd916b90ff5f8b2cdcae9758938d84d460891b2084e615b8",
            "result_hash byte-format changed unexpectedly"
        );
    }

    mod feed_disclosure {
        use std::collections::{BTreeSet, HashMap};

        use crate::egregore::telemetry::TelemetryId;
        use crate::egregore::TraceParent;

        use serde_json::{json, Value};
        use sha2::{Digest, Sha256};

        use chrono::Utc;

        use super::super::execute_direct;
        use crate::authority::{Authority, AuthorityConfig};
        use crate::config::{AgentConfig, Config};
        use crate::egregore::{telemetry, PlannedToolCall, Task};
        use crate::identity::Identity;
        use crate::mcp::{McpClient, McpPool};
        use crate::scope::ScopeEnforcer;
        use crate::test_support::{assert_same_bytes, CaptureNode, ScriptedClient, SecretGen};

        const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
        const PARENT_SPAN_ID: &str = "00f067aa0ba902b7";

        /// Tracing configurations to cover. One that fails to load is not a
        /// supported deployment and is skipped; plain tracing must load.
        const TRACING_CONFIGS: &[&str] = &[
            "[agent]\npublish_trace_spans = true\n",
            "[agent]\npublish_trace_spans = true\ntrace_span_arguments = false\n",
            "[agent]\npublish_trace_spans = true\ntrace_span_arguments = true\n",
        ];

        #[derive(Debug, Clone, Copy)]
        enum Outcome {
            Output,
            ToolError,
            CallError,
        }

        fn loadable_tracing_configs() -> Vec<(&'static str, AgentConfig)> {
            let configs: Vec<_> = TRACING_CONFIGS
                .iter()
                .filter_map(|toml| Some((*toml, Config::from_str(toml).ok()?.agent)))
                .collect();
            assert!(
                configs.iter().any(|(toml, _)| *toml == TRACING_CONFIGS[0]),
                "plain tracing must load"
            );
            configs
        }

        /// A task as `servitor exec` builds one: its hash derives from the
        /// planned call, so it is payload rather than public metadata. Its
        /// trace context is public and held fixed.
        fn exec_task(gen: &mut SecretGen, context: HashMap<String, Value>) -> Task {
            let call = PlannedToolCall {
                id: "call-1".to_string(),
                name: "shell_execute".to_string(),
                arguments: gen.arguments(),
            };
            let mut hasher = Sha256::new();
            hasher.update(call.name.as_bytes());
            hasher.update(serde_json::to_vec(&call.arguments).unwrap());
            let hash: String = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Task {
                msg_type: "task".to_string(),
                id: None,
                hash,
                task_type: None,
                request: Some(gen.secret()),
                requestor: None,
                prompt: gen.secret(),
                required_caps: vec![],
                parent_id: None,
                context,
                priority: 0,
                timeout_secs: None,
                author: None,
                keeper: None,
                scope_override: None,
                tool_calls: vec![call],
                depends_on: vec![],
                trace_parent: None,
            }
        }

        /// Attach the trace context an authorized request's envelope carries.
        fn with_public_trace(mut task: Task) -> Task {
            task.trace_parent = Some(TraceParent {
                trace_id: TelemetryId::inherit(TRACE_ID).unwrap(),
                span_id: TelemetryId::inherit(PARENT_SPAN_ID),
            });
            task
        }

        fn scripted(outcome: Outcome, gen: &mut SecretGen) -> Box<dyn McpClient> {
            match outcome {
                Outcome::Output => ScriptedClient::output(gen.secret()),
                Outcome::ToolError => ScriptedClient::tool_error(gen.secret()),
                Outcome::CallError => ScriptedClient::call_error(gen.secret()),
            }
        }

        thread_local! {
            static SERVITOR: Identity = Identity::generate();
        }

        /// Execute `task` against a `shell_execute` tool answering with
        /// `client`, as one fixed servitor, returning the published
        /// `trace_span` bodies.
        async fn published_spans(
            node: &CaptureNode,
            config: &AgentConfig,
            task: &Task,
            client: Box<dyn McpClient>,
        ) -> Vec<Vec<u8>> {
            let mut pool = McpPool::new();
            pool.register_test_tool("shell", "execute", client);
            let identity = SERVITOR.with(Identity::clone);
            node.clear();
            execute_direct(
                task,
                &pool,
                &ScopeEnforcer::new(),
                &identity,
                config,
                Some(&node.client),
                None,
                None,
            )
            .await
            .unwrap();
            node.bodies_of_type("trace_span")
        }

        /// Information-flow property: with public metadata held fixed (tool,
        /// call count, trace context, outcome, clock), no change to the
        /// arguments, output, error text or prompt changes the published
        /// span bytes, under every tracing configuration that loads.
        #[tokio::test]
        async fn trace_span_bytes_ignore_payloads() {
            let node = CaptureNode::start().await;
            let at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            for (toml, config) in loadable_tracing_configs() {
                for outcome in [Outcome::Output, Outcome::ToolError, Outcome::CallError] {
                    let mut reference: Option<Vec<Vec<u8>>> = None;
                    for seed in 0..32 {
                        let mut gen = SecretGen::new(seed);
                        let task = with_public_trace(exec_task(&mut gen, HashMap::new()));
                        let client = scripted(outcome, &mut gen);
                        let _frozen = telemetry::freeze(at);
                        let spans = published_spans(&node, &config, &task, client).await;

                        let label = format!("{toml:?} {outcome:?} seed {seed}");
                        assert_eq!(spans.len(), 2, "{label}");
                        match &reference {
                            None => reference = Some(spans),
                            Some(expected) => {
                                for (expected, actual) in expected.iter().zip(&spans) {
                                    assert_same_bytes(expected, actual, &label);
                                }
                            }
                        }
                    }
                    for span in reference.unwrap() {
                        let span = String::from_utf8(span).unwrap();
                        assert!(!span.contains(SecretGen::CANARY), "{span}");
                    }
                }
            }
        }

        fn keys(value: &Value) -> BTreeSet<&str> {
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect()
        }

        fn is_generated_id(value: &Value) -> bool {
            value.as_str().is_some_and(|id| {
                id.len() == 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }

        /// Structural check of everything a published span carries: exact
        /// keys at every level, and each value from its approved domain.
        #[tokio::test]
        async fn spans_carry_exactly_the_approved_metadata() {
            let node = CaptureNode::start().await;
            let config = Config::from_str(TRACING_CONFIGS[0]).unwrap().agent;
            let task = with_public_trace(exec_task(&mut SecretGen::new(7), HashMap::new()));
            let spans = published_spans(&node, &config, &task, ScriptedClient::output("ok")).await;
            let [tool, root] =
                [&spans[0], &spans[1]].map(|body| serde_json::from_slice::<Value>(body).unwrap());

            let envelope_keys = BTreeSet::from(["content", "span_id", "tags", "trace_id"]);
            let span_keys = BTreeSet::from([
                "attributes",
                "end_ts",
                "name",
                "parent_span_id",
                "service",
                "span_id",
                "start_ts",
                "status",
                "trace_id",
                "type",
            ]);
            for envelope in [&tool, &root] {
                assert_eq!(keys(envelope), envelope_keys);
                let span = &envelope["content"];
                assert_eq!(keys(span), span_keys);
                assert_eq!(span["type"], "trace_span");
                assert_eq!(span["service"], "servitor");
                assert_eq!(span["status"], "ok");
                assert_eq!(span["trace_id"], TRACE_ID);
                assert_eq!(envelope["trace_id"], TRACE_ID);
                assert!(is_generated_id(&span["span_id"]), "{span}");
                assert_eq!(envelope["span_id"], span["span_id"]);
                assert_eq!(envelope["tags"], json!(["trace_span", "ok"]));
                for ts in ["start_ts", "end_ts"] {
                    let ts = span[ts].as_str().unwrap();
                    assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok(), "{ts}");
                }
            }

            let (tool, root) = (&tool["content"], &root["content"]);
            assert_eq!(tool["name"], "direct:shell:execute");
            assert_eq!(tool["parent_span_id"], root["span_id"]);
            assert_eq!(
                keys(&tool["attributes"]),
                BTreeSet::from([
                    "call_index",
                    "duration_ms",
                    "execution_id",
                    "outcome",
                    "provider",
                    "tool"
                ])
            );
            let attributes = &tool["attributes"];
            assert_eq!(attributes["provider"], "shell");
            assert_eq!(attributes["tool"], "execute");
            assert_eq!(attributes["call_index"], 0);
            assert_eq!(attributes["outcome"], "ok");
            assert!(attributes["duration_ms"].is_u64());
            assert!(is_generated_id(&attributes["execution_id"]));

            assert_eq!(root["name"], "direct_execution");
            assert_eq!(root["parent_span_id"], PARENT_SPAN_ID);
            assert_eq!(
                keys(&root["attributes"]),
                BTreeSet::from(["duration_ms", "execution_id", "mode", "tool_count"])
            );
            assert_eq!(
                root["attributes"]["execution_id"],
                attributes["execution_id"]
            );
            assert_eq!(root["attributes"]["mode"], "direct");
            assert_eq!(root["attributes"]["tool_count"], 1);
            assert!(root["attributes"]["duration_ms"].is_u64());
        }

        /// The pool's schema check is Servitor's own error and quotes the
        /// offending argument values; the published result names the
        /// failure without them.
        #[tokio::test]
        async fn schema_rejection_result_does_not_quote_arguments() {
            let node = CaptureNode::start().await;
            let canary = SecretGen::CANARY;
            let mut pool = McpPool::new();
            pool.register_test_tool_with_schema(
                "shell",
                "execute",
                Some(json!({
                    "type": "object",
                    "properties": { "command": { "type": "integer" } }
                })),
                ScriptedClient::output("ran"),
            );
            let mut task = exec_task(&mut SecretGen::new(5), HashMap::new());
            task.tool_calls[0].arguments = json!({ "command": format!("mysql -p{canary}") });

            let result = execute_direct(
                &task,
                &pool,
                &ScopeEnforcer::new(),
                &Identity::generate(),
                &AgentConfig::default(),
                Some(&node.client),
                None,
                None,
            )
            .await
            .unwrap();
            node.client.publish_result(&result).await.unwrap();

            let error = result.error.unwrap();
            assert!(error.contains("invalid_arguments"), "{error}");
            for body in node.bodies() {
                let body = String::from_utf8_lossy(&body);
                assert!(!body.contains(canary), "{body}");
            }
        }

        /// A failed call's error text comes from the transport or the server,
        /// so the published result carries a code and the text stays local.
        #[tokio::test]
        async fn call_failure_result_publishes_a_code_not_the_error_text() {
            let node = CaptureNode::start().await;
            let canary = SecretGen::CANARY;
            let mut pool = McpPool::new();
            pool.register_test_tool(
                "shell",
                "execute",
                ScriptedClient::call_error(format!("rpc failed at http://u:{canary}@host/x")),
            );
            let task = exec_task(&mut SecretGen::new(6), HashMap::new());

            let result = execute_direct(
                &task,
                &pool,
                &ScopeEnforcer::new(),
                &Identity::generate(),
                &AgentConfig::default(),
                Some(&node.client),
                None,
                None,
            )
            .await
            .unwrap();
            node.client.publish_result(&result).await.unwrap();

            assert_eq!(
                result.error.as_deref(),
                Some("tool 'shell_execute' failed: mcp_error")
            );
            for body in node.bodies() {
                let body = String::from_utf8_lossy(&body);
                assert!(!body.contains(canary), "{body}");
            }
        }

        /// A tool the operator has not approved for publication runs, but
        /// its spans and failure result name it `unlisted`.
        #[tokio::test]
        async fn unapproved_tool_publishes_as_unlisted() {
            let node = CaptureNode::start().await;
            let config = Config::from_str(TRACING_CONFIGS[0]).unwrap().agent;
            let mut pool = McpPool::new();
            pool.approve_server("shell", crate::public::PublicApproval::default());
            pool.register_test_tool("shell", "execute", ScriptedClient::call_error("down"));
            let task = with_public_trace(exec_task(&mut SecretGen::new(8), HashMap::new()));

            let result = execute_direct(
                &task,
                &pool,
                &ScopeEnforcer::new(),
                &SERVITOR.with(Identity::clone),
                &config,
                Some(&node.client),
                None,
                None,
            )
            .await
            .unwrap();

            assert_eq!(
                result.error.as_deref(),
                Some("tool 'shell_unlisted' failed: mcp_error")
            );
            let spans: Vec<Value> = node
                .bodies_of_type("trace_span")
                .iter()
                .map(|body| serde_json::from_slice::<Value>(body).unwrap()["content"].clone())
                .collect();
            let tool_span = spans
                .iter()
                .find(|span| span["name"] != "direct:task")
                .expect("tool span");
            assert_eq!(tool_span["name"], "direct:shell:unlisted");
            for span in &spans {
                assert!(!span.to_string().contains("execute"), "{span}");
            }
        }

        /// Publication approval names what the feed may carry; it never
        /// changes what a keeper is authorized to run.
        #[tokio::test]
        async fn authorization_ignores_publication_approval() {
            let authority = Authority::from_config(
                AuthorityConfig::from_toml(
                    r#"
[[keeper]]
name = "operator"
egregore = "@operator.ed25519"

[[permission]]
keeper = "operator"
skills = ["shell:execute"]
"#,
                )
                .unwrap(),
            );
            let mut pool = McpPool::new();
            pool.approve_server("shell", crate::public::PublicApproval::default());
            pool.register_test_tool("shell", "execute", ScriptedClient::output("ran"));
            let task = exec_task(&mut SecretGen::new(9), HashMap::new());

            let result = execute_direct(
                &task,
                &pool,
                &ScopeEnforcer::new(),
                &Identity::generate(),
                &AgentConfig::default(),
                None,
                Some(&authority),
                Some("operator"),
            )
            .await
            .unwrap();

            assert!(result.error.is_none(), "{:?}", result.error);
        }

        /// The published result names an unapproved tool `unlisted` on every
        /// outcome, with tracing off.
        #[tokio::test]
        async fn results_name_unapproved_tools_unlisted() {
            let node = CaptureNode::start().await;
            for client in [
                ScriptedClient::output("ran"),
                ScriptedClient::tool_error("no"),
                ScriptedClient::call_error("down"),
            ] {
                let mut pool = McpPool::new();
                pool.approve_server("shell", crate::public::PublicApproval::default());
                pool.register_test_tool("shell", "execute", client);
                let task = exec_task(&mut SecretGen::new(10), HashMap::new());

                let result = execute_direct(
                    &task,
                    &pool,
                    &ScopeEnforcer::new(),
                    &Identity::generate(),
                    &AgentConfig::default(),
                    Some(&node.client),
                    None,
                    None,
                )
                .await
                .unwrap();

                let mut published = serde_json::to_value(&result).unwrap();
                // The task's own identity fields are the requester's; the
                // check is on what execution added.
                published["task_hash"] = Value::Null;
                let text = published.to_string();
                assert!(!text.contains("execute"), "{text}");
                assert!(text.contains("shell_unlisted"), "{text}");
            }
        }

        /// Trace ids written into task content are ignored, so they are
        /// never published.
        #[tokio::test]
        async fn malformed_inherited_trace_ids_are_not_published() {
            let node = CaptureNode::start().await;
            let config = Config::from_str(TRACING_CONFIGS[0]).unwrap().agent;
            let canary = SecretGen::CANARY;
            let context = HashMap::from([
                ("trace_id".to_string(), json!(format!("trace {canary}"))),
                ("span_id".to_string(), json!(canary)),
            ]);
            let task = exec_task(&mut SecretGen::new(3), context);

            let spans = published_spans(&node, &config, &task, ScriptedClient::output("ok")).await;

            assert_eq!(spans.len(), 2);
            for body in &spans {
                let text = String::from_utf8_lossy(body);
                assert!(!text.contains(canary), "{text}");
                let span = &serde_json::from_slice::<Value>(body).unwrap()["content"];
                assert!(is_generated_id(&span["trace_id"]), "{span}");
            }
            let root = &serde_json::from_slice::<Value>(&spans[1]).unwrap()["content"];
            assert!(root.get("parent_span_id").is_none(), "{root}");
        }
    }
}
