//! Agent loop — tool_use → execute → feed_back cycle.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::agent::context::ConversationContext;
use crate::agent::provider::{ContentBlock, Provider, StopReason};
use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{
    Attestation, AuthDenied, AuthGate, Task, TaskResult, TaskStatus, TraceEvent, TraceSpan,
    TraceSpanStatus,
};
use crate::egregore::EgregoreClient;
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Agent executor — runs the tool_use loop for a task.
pub struct AgentExecutor<'a> {
    provider: &'a dyn Provider,
    mcp_pool: &'a McpPool,
    scope_enforcer: &'a ScopeEnforcer,
    identity: &'a Identity,
    config: &'a AgentConfig,
    egregore: Option<&'a EgregoreClient>,
    authority: Option<&'a Authority>,
    keeper_name: Option<String>,
}

impl<'a> AgentExecutor<'a> {
    pub fn new(
        provider: &'a dyn Provider,
        mcp_pool: &'a McpPool,
        scope_enforcer: &'a ScopeEnforcer,
        identity: &'a Identity,
        config: &'a AgentConfig,
    ) -> Self {
        Self {
            provider,
            mcp_pool,
            scope_enforcer,
            identity,
            config,
            egregore: None,
            authority: None,
            keeper_name: None,
        }
    }

    /// Set egregore client for context fetching.
    pub fn with_egregore(mut self, egregore: &'a EgregoreClient) -> Self {
        self.egregore = Some(egregore);
        self
    }

    /// Set authority for skill permission checks.
    pub fn with_authority(mut self, authority: &'a Authority, keeper_name: Option<String>) -> Self {
        self.authority = Some(authority);
        self.keeper_name = keeper_name;
        self
    }

    /// Execute a task and return the signed result.
    pub async fn execute(&self, task: &Task) -> Result<TaskResult> {
        let trace_enabled = self.config.publish_trace_spans && self.egregore.is_some();
        let trace_id = trace_enabled.then(new_trace_id);
        let root_span_id = trace_enabled.then(new_span_id);
        let trace_service = "servitor";
        let trace_started_at = trace_enabled.then(Utc::now);
        let mut context = ConversationContext::new();
        let tools = self.mcp_pool.tools_for_llm();

        // Fetch conversation history if task has parent_id and egregore is available
        if let (Some(parent_id), Some(egregore)) = (&task.parent_id, self.egregore) {
            match egregore.fetch_conversation_history(parent_id).await {
                Ok(history) if !history.is_empty() => {
                    tracing::debug!(
                        parent_id = %parent_id,
                        turns = history.len(),
                        "loaded conversation history"
                    );
                    context.prepend_history(history);
                }
                Ok(_) => {
                    tracing::debug!(parent_id = %parent_id, "no conversation history found");
                }
                Err(e) => {
                    tracing::warn!(
                        parent_id = %parent_id,
                        error = %e,
                        "failed to fetch conversation history, proceeding without"
                    );
                }
            }
        }

        // Build system prompt
        let system = self.build_system_prompt(task);

        // Add the task as the initial user message
        context.add_user_message(&task.prompt);

        let mut turn = 0;
        let max_turns = self.config.max_turns;

        loop {
            if turn >= max_turns {
                tracing::warn!(turns = turn, "max turns reached");
                let result = self.build_result(
                    task,
                    TaskStatus::Timeout,
                    None,
                    Some(format!("max turns ({}) exceeded", max_turns)),
                    trace_id.clone(),
                )?;
                if let (Some(trace_id), Some(root_span_id), Some(trace_started_at)) =
                    (&trace_id, &root_span_id, trace_started_at)
                {
                    self.publish_root_trace(
                        task,
                        trace_id,
                        root_span_id,
                        trace_service,
                        trace_started_at,
                        &result,
                    )
                    .await;
                }
                return Ok(result);
            }

            // Call LLM
            let response = self
                .provider
                .chat(&system, context.messages(), &tools)
                .await?;

            tracing::debug!(
                turn = turn,
                stop_reason = ?response.stop_reason,
                tool_uses = response.tool_uses().len(),
                "LLM response"
            );

            // Add assistant response to context
            context.add_assistant_message(response.content.clone());

            // Check stop condition
            if response.stop_reason == StopReason::EndTurn {
                // Task complete
                let result_text = response.text();
                let result = self.build_result(
                    task,
                    TaskStatus::Success,
                    Some(serde_json::json!({ "text": result_text })),
                    None,
                    trace_id.clone(),
                )?;
                if let (Some(trace_id), Some(root_span_id), Some(trace_started_at)) =
                    (&trace_id, &root_span_id, trace_started_at)
                {
                    self.publish_root_trace(
                        task,
                        trace_id,
                        root_span_id,
                        trace_service,
                        trace_started_at,
                        &result,
                    )
                    .await;
                }
                return Ok(result);
            }

            // Extract and execute tool calls
            let tool_uses = response.tool_uses();
            if tool_uses.is_empty() {
                // No tools to execute, treat as end
                let result_text = response.text();
                let result = self.build_result(
                    task,
                    TaskStatus::Success,
                    Some(serde_json::json!({ "text": result_text })),
                    None,
                    trace_id.clone(),
                )?;
                if let (Some(trace_id), Some(root_span_id), Some(trace_started_at)) =
                    (&trace_id, &root_span_id, trace_started_at)
                {
                    self.publish_root_trace(
                        task,
                        trace_id,
                        root_span_id,
                        trace_service,
                        trace_started_at,
                        &result,
                    )
                    .await;
                }
                return Ok(result);
            }

            // Execute each tool call
            let mut tool_results = Vec::new();
            for (tool_id, tool_name, arguments) in tool_uses {
                let result = self.execute_tool(task, tool_name, arguments).await;
                if let (Some(trace_id), Some(root_span_id)) = (&trace_id, &root_span_id) {
                    let tool_span_started_at = Utc::now();
                    let tool_span_id = new_span_id();
                    let (mcp_name, bare_tool_name) = self
                        .mcp_pool
                        .parse_tool_name(tool_name)
                        .map(|(mcp, tool)| (mcp.to_string(), tool.to_string()))
                        .unwrap_or_else(|| ("unknown".to_string(), tool_name.to_string()));
                    let tool_span = self.build_tool_trace_span(
                        trace_id,
                        root_span_id,
                        trace_service,
                        &tool_span_id,
                        &mcp_name,
                        &bare_tool_name,
                        tool_span_started_at,
                        &result,
                    );
                    self.publish_trace_span(&tool_span).await;
                }
                tool_results.push(match result {
                    Ok(output) => {
                        ContentBlock::tool_result(tool_id, output.text_content(), output.is_error)
                    }
                    Err(e) => ContentBlock::tool_result(tool_id, e.to_string(), true),
                });
            }

            // Feed tool results back
            context.add_tool_results(tool_results);
            turn += 1;
        }
    }

    /// Execute a single tool call.
    async fn execute_tool(
        &self,
        task: &Task,
        prefixed_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<crate::mcp::ToolCallResult> {
        // Parse the prefixed tool name
        let (mcp_name, tool_name) =
            self.mcp_pool
                .parse_tool_name(prefixed_name)
                .ok_or_else(|| ServitorError::Mcp {
                    reason: format!("unknown tool: {}", prefixed_name),
                })?;

        // Check authority skill permission if configured
        if let (Some(authority), Some(keeper_name)) = (self.authority, &self.keeper_name) {
            let skill = format!("{}:{}", mcp_name, tool_name);
            let auth_result = authority.authorize_skill(keeper_name, &skill);
            if !auth_result.allowed {
                self.publish_assignment_denial(task, &skill, &auth_result.reason)
                    .await;
                return Err(ServitorError::Unauthorized {
                    reason: format!(
                        "keeper '{}' not authorized for skill '{}': {}",
                        keeper_name, skill, auth_result.reason
                    ),
                });
            }
        }

        // Check scope enforcement (existing allow/block patterns)
        self.scope_enforcer
            .check(mcp_name, tool_name, arguments, task.scope_override.as_ref())
            .map_err(|error| match error {
                ServitorError::ScopeViolation { reason } => ServitorError::ScopeViolation {
                    reason: format!("task '{}' scope violation: {}", task.hash, reason),
                },
                other => other,
            })?;

        tracing::debug!(mcp = mcp_name, tool = tool_name, "executing tool");

        // Execute the tool
        self.mcp_pool
            .call_tool(prefixed_name, arguments.clone())
            .await
    }

    async fn publish_assignment_denial(&self, task: &Task, skill: &str, reason: &str) {
        let Some(egregore) = self.egregore else {
            return;
        };

        let denial = AuthDenied::new(
            self.identity.public_id(),
            task_person_id(task),
            task_place(task),
            skill.to_string(),
            AuthGate::Assignment,
            reason.to_string(),
        );

        if let Err(error) = egregore.publish_auth_denied(&denial).await {
            tracing::debug!(error = %error, skill = %skill, "failed to publish auth denial");
        }
    }

    /// Build the system prompt for the task.
    fn build_system_prompt(&self, task: &Task) -> String {
        let mut prompt = String::new();

        if let Some(ref custom) = self.config.system_prompt {
            prompt.push_str(custom);
            prompt.push_str("\n\n");
        }

        prompt.push_str(
            "You are a Servitor — a task executor in the egregore network. \
             Execute the user's task using the available tools. \
             Be concise and focused. When the task is complete, provide a brief summary.\n\n",
        );

        // Add context from task if available
        if !task.context.is_empty() {
            prompt.push_str("Context:\n");
            for (key, value) in &task.context {
                prompt.push_str(&format!("- {}: {}\n", key, value));
            }
            prompt.push('\n');
        }

        prompt
    }

    /// Build a TaskResult with signed attestation.
    fn build_result(
        &self,
        task: &Task,
        status: TaskStatus,
        result: Option<serde_json::Value>,
        error: Option<String>,
        trace_id: Option<String>,
    ) -> Result<TaskResult> {
        // Compute result hash
        let result_hash = compute_result_hash(&result, &error, trace_id.as_deref());

        // Sign the result hash
        let signature = self.identity.sign_hash(&result_hash);

        let attestation = Attestation {
            servitor_id: self.identity.public_id(),
            signature,
            timestamp: Utc::now(),
        };

        Ok(TaskResult {
            msg_type: "task_result".to_string(),
            task_id: task.effective_id().to_string(),
            servitor: self.identity.public_id(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            task_hash: task.hash.clone(),
            result_hash,
            status,
            result,
            error,
            duration_seconds: None,
            attestation,
            trace_id,
        })
    }

    fn build_tool_trace_span(
        &self,
        trace_id: &str,
        parent_span_id: &str,
        service: &str,
        span_id: &str,
        mcp_name: &str,
        tool_name: &str,
        start_ts: DateTime<Utc>,
        result: &Result<crate::mcp::ToolCallResult>,
    ) -> TraceSpan {
        let mut span = TraceSpan::new(
            trace_id.to_string(),
            span_id.to_string(),
            Some(parent_span_id.to_string()),
            format!("mcp:{mcp_name}:{tool_name}"),
            service.to_string(),
            start_ts,
            Utc::now(),
            match result {
                Ok(output) if !output.is_error => TraceSpanStatus::Ok,
                Ok(_) | Err(_) => TraceSpanStatus::Error,
            },
        );
        span.attributes.insert(
            "mcp_server".to_string(),
            serde_json::Value::String(mcp_name.to_string()),
        );
        span.attributes.insert(
            "tool_name".to_string(),
            serde_json::Value::String(tool_name.to_string()),
        );
        match result {
            Ok(output) => {
                span.attributes.insert(
                    "is_error".to_string(),
                    serde_json::Value::Bool(output.is_error),
                );
            }
            Err(error) => {
                span.events.push(trace_error_event(error.to_string()));
            }
        }
        span
    }

    async fn publish_trace_span(&self, span: &TraceSpan) {
        if let Some(egregore) = self.egregore {
            if let Err(error) = egregore.publish_trace_span(span).await {
                tracing::debug!(
                    error = %error,
                    trace_id = %span.trace_id,
                    span_id = %span.span_id,
                    "failed to publish trace span"
                );
            }
        }
    }

    async fn publish_root_trace(
        &self,
        task: &Task,
        trace_id: &str,
        root_span_id: &str,
        service: &str,
        start_ts: DateTime<Utc>,
        result: &TaskResult,
    ) {
        let mut span = TraceSpan::new(
            trace_id.to_string(),
            root_span_id.to_string(),
            None,
            "task_execution".to_string(),
            service.to_string(),
            start_ts,
            Utc::now(),
            match result.status {
                TaskStatus::Success => TraceSpanStatus::Ok,
                TaskStatus::Error => TraceSpanStatus::Error,
                TaskStatus::Timeout => TraceSpanStatus::Timeout,
            },
        );
        span.attributes.insert(
            "task_hash".to_string(),
            serde_json::Value::String(task.hash.clone()),
        );
        if let Some(parent_id) = &task.parent_id {
            span.attributes.insert(
                "parent_message_id".to_string(),
                serde_json::Value::String(parent_id.clone()),
            );
        }
        if let Some(error) = &result.error {
            span.events.push(trace_error_event(error.clone()));
        }
        self.publish_trace_span(&span).await;
    }
}

/// Compute SHA-256 hash of the result content.
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
    if let Some(trace_id) = trace_id {
        hasher.update(trace_id.as_bytes());
    }

    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_trace_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn new_span_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn trace_error_event(message: String) -> TraceEvent {
    let mut attributes = std::collections::HashMap::new();
    attributes.insert("message".to_string(), serde_json::Value::String(message));
    TraceEvent {
        ts: Utc::now(),
        name: "error".to_string(),
        attributes,
    }
}

fn task_person_id(task: &Task) -> String {
    if let Some(author) = &task.author {
        return author.clone();
    }

    if let Some(user_id) = task
        .context
        .get("user")
        .and_then(|value| value.get("id"))
        .and_then(|value| value.as_str())
    {
        return format!("discord:{user_id}");
    }

    if let Some(keeper) = &task.keeper {
        return format!("keeper:{keeper}");
    }

    "unknown".to_string()
}

fn task_place(task: &Task) -> String {
    let source = task
        .context
        .get("source")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let channel = task
        .context
        .get("channel")
        .and_then(|value| value.as_str())
        .map(str::to_string);

    match (source, channel) {
        (Some(source), Some(channel)) => format!("{source}:{channel}"),
        (Some(source), None) => source,
        (None, _) if task.author.is_some() => "egregore:task".to_string(),
        _ => "direct:exec".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_hash_deterministic() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None, None);
        let h2 = compute_result_hash(&result, &None, None);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn different_content_different_hash() {
        let r1 = Some(serde_json::json!({"foo": "bar"}));
        let r2 = Some(serde_json::json!({"foo": "baz"}));
        let h1 = compute_result_hash(&r1, &None, None);
        let h2 = compute_result_hash(&r2, &None, None);
        assert_ne!(h1, h2);
    }

    #[test]
    fn trace_id_changes_result_hash() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None, Some("trace-a"));
        let h2 = compute_result_hash(&result, &None, Some("trace-b"));
        assert_ne!(h1, h2);
    }
}
