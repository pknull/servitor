//! Agent loop — tool_use → execute → feed_back cycle.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::a2a::A2aPool;
use crate::agent::context::ConversationContext;
use crate::agent::provider::{ContentBlock, Provider, StopReason};
use crate::agent::sanitize::{sanitize_arguments, sanitize_tool_result};
use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{
    Attestation, AuthDenied, AuthGate, PlannedToolCall, Task, TaskPlan, TaskResult, TaskStatus,
    TraceEvent, TraceSpan, TraceSpanStatus,
};
use crate::egregore::EgregoreClient;
use crate::error::{Result, ServitorError};
use crate::identity::Identity;
use crate::mcp::{LlmTool, McpPool};
use crate::metrics::{self, Timer, ToolCallStatus};
use crate::scope::ScopeEnforcer;

/// Parameters for publishing A2A delegation spans.
struct A2aDelegationParams<'a> {
    agent_name: &'a str,
    skill_name: &'a str,
    task_id: &'a str,
    arguments: &'a serde_json::Value,
    start_ts: DateTime<Utc>,
    end_ts: DateTime<Utc>,
    success: bool,
}

/// Agent executor — runs the tool_use loop for a task.
pub struct AgentExecutor<'a> {
    provider: &'a dyn Provider,
    mcp_pool: &'a McpPool,
    a2a_pool: Option<&'a A2aPool>,
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
            a2a_pool: None,
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

    /// Set A2A pool for external agent delegation.
    pub fn with_a2a_pool(mut self, a2a_pool: &'a A2aPool) -> Self {
        self.a2a_pool = Some(a2a_pool);
        self
    }

    /// Set authority for skill permission checks.
    pub fn with_authority(mut self, authority: &'a Authority, keeper_name: Option<String>) -> Self {
        self.authority = Some(authority);
        self.keeper_name = keeper_name;
        self
    }

    /// Get all tools (MCP + A2A) for LLM consumption.
    fn all_tools_for_llm(&self) -> Vec<LlmTool> {
        let mut tools = self.mcp_pool.tools_for_llm();
        if let Some(a2a_pool) = self.a2a_pool {
            tools.extend(a2a_pool.tools_for_llm());
        }
        tools
    }

    /// Check if a tool belongs to the A2A pool.
    fn is_a2a_tool(&self, prefixed_name: &str) -> bool {
        self.a2a_pool
            .map(|pool| pool.has_tool(prefixed_name))
            .unwrap_or(false)
    }

    /// Parse a prefixed tool name into (provider_name, tool_name).
    /// Checks both MCP and A2A pools.
    fn parse_tool_name<'b>(&'b self, prefixed_name: &'b str) -> Option<(&'b str, &'b str)> {
        // Try MCP pool first
        if let Some(result) = self.mcp_pool.parse_tool_name(prefixed_name) {
            return Some(result);
        }
        // Try A2A pool
        if let Some(a2a_pool) = self.a2a_pool {
            return a2a_pool.parse_tool_name(prefixed_name);
        }
        None
    }

    /// Produce and validate a signed plan artifact without executing tools.
    pub async fn plan(&self, task: &Task) -> Result<TaskPlan> {
        let mut context = self.load_context(task).await?;
        let tools = self.all_tools_for_llm();

        context.add_user_message(&task.prompt);

        let response = self
            .provider
            .chat(&self.build_plan_prompt(task), context.messages(), &tools)
            .await?;

        let planned_calls = response
            .tool_uses()
            .into_iter()
            .map(|(id, name, arguments)| PlannedToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.clone(),
            })
            .collect::<Vec<_>>();

        for planned_call in &planned_calls {
            self.validate_tool_call(task, &planned_call.name, &planned_call.arguments, &tools)?;
        }

        self.build_plan(task, &response, planned_calls)
    }

    /// Execute a task and return the signed result.
    pub async fn execute(&self, task: &Task) -> Result<TaskResult> {
        self.execute_with_plan_hash(task, None).await
    }

    /// Execute a task and optionally bind the result attestation to a published plan.
    pub async fn execute_with_plan_hash(
        &self,
        task: &Task,
        plan_hash: Option<String>,
    ) -> Result<TaskResult> {
        let trace_enabled = self.config.publish_trace_spans && self.egregore.is_some();
        let trace_id = trace_enabled.then(new_trace_id);
        let root_span_id = trace_enabled.then(new_span_id);
        let trace_service = "servitor";
        let trace_started_at = trace_enabled.then(Utc::now);
        let mut context = self.load_context(task).await?;
        let tools = self.all_tools_for_llm();

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
                    plan_hash.as_deref(),
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

            // Call LLM with timing
            let llm_timer = Timer::start();
            let response = self
                .provider
                .chat(&system, context.messages(), &tools)
                .await?;
            metrics::record_llm_latency(self.provider.name(), llm_timer.elapsed_secs());

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
                    plan_hash.as_deref(),
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
                    plan_hash.as_deref(),
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
                    let (provider_name, bare_tool_name) = self
                        .parse_tool_name(tool_name)
                        .map(|(provider, tool)| (provider.to_string(), tool.to_string()))
                        .unwrap_or_else(|| ("unknown".to_string(), tool_name.to_string()));
                    let tool_span = self.build_tool_trace_span(
                        trace_id,
                        root_span_id,
                        trace_service,
                        &tool_span_id,
                        &provider_name,
                        &bare_tool_name,
                        arguments,
                        tool_span_started_at,
                        &result,
                    );
                    self.publish_trace_span(&tool_span).await;
                }
                tool_results.push(match result {
                    Ok(output) => {
                        // Sanitize tool result to redact sensitive content before feeding back to LLM
                        let sanitized_content = sanitize_tool_result(&output.text_content());
                        ContentBlock::tool_result(tool_id, sanitized_content, output.is_error)
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
        let (provider_name, tool_name) = parse_prefixed_tool_name(prefixed_name)?;
        let skill = format!("{}:{}", provider_name, tool_name);

        if let Err(error) =
            self.validate_tool_call(task, prefixed_name, arguments, &self.all_tools_for_llm())
        {
            // Record metrics for validation failures
            let status = match &error {
                ServitorError::Unauthorized { .. } => {
                    self.publish_assignment_denial(task, &skill, &error.to_string())
                        .await;
                    ToolCallStatus::Unauthorized
                }
                ServitorError::ScopeViolation { .. } => ToolCallStatus::ScopeViolation,
                _ => ToolCallStatus::Error,
            };
            metrics::record_tool_call(tool_name, provider_name, status);
            return Err(error);
        }

        // Check if this is an A2A tool
        if self.is_a2a_tool(prefixed_name) {
            return self.execute_a2a_tool(task, prefixed_name, arguments).await;
        }

        tracing::debug!(mcp = provider_name, tool = tool_name, "executing MCP tool");

        // Execute the MCP tool with timing
        let timer = Timer::start();
        let result = self
            .mcp_pool
            .call_tool(prefixed_name, arguments.clone())
            .await;
        let duration = timer.elapsed_secs();

        // Record metrics
        let status = match &result {
            Ok(output) if !output.is_error => ToolCallStatus::Success,
            _ => ToolCallStatus::Error,
        };
        metrics::record_tool_call(tool_name, provider_name, status);
        metrics::record_tool_call_duration(tool_name, duration);

        result
    }

    /// Execute an A2A tool call (delegation to external agent).
    async fn execute_a2a_tool(
        &self,
        task: &Task,
        prefixed_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<crate::mcp::ToolCallResult> {
        let Some(a2a_pool) = self.a2a_pool else {
            return Err(ServitorError::Mcp {
                reason: "A2A pool not configured".into(),
            });
        };

        let (agent_name, skill_name) =
            a2a_pool
                .parse_tool_name(prefixed_name)
                .ok_or_else(|| ServitorError::Mcp {
                    reason: format!("unknown A2A tool: {}", prefixed_name),
                })?;

        tracing::debug!(
            agent = agent_name,
            skill = skill_name,
            "executing A2A delegation"
        );

        // Execute the A2A skill with timing
        let start_ts = Utc::now();
        let timer = Timer::start();
        let result = a2a_pool
            .execute_skill(prefixed_name, arguments.clone())
            .await;
        let duration = timer.elapsed_secs();
        let end_ts = Utc::now();

        // Record metrics
        let success = result.is_ok();
        let status = if success {
            ToolCallStatus::Success
        } else {
            ToolCallStatus::Error
        };
        metrics::record_tool_call(skill_name, agent_name, status);
        metrics::record_tool_call_duration(skill_name, duration);

        // Generate a task ID for the delegation span
        let task_id = format!(
            "{}-{}",
            task.hash.chars().take(8).collect::<String>(),
            start_ts.timestamp_millis()
        );

        // Publish delegation span to egregore (after execution to capture outcome)
        self.publish_a2a_delegation(
            task,
            A2aDelegationParams {
                agent_name,
                skill_name,
                task_id: &task_id,
                arguments,
                start_ts,
                end_ts,
                success,
            },
        )
        .await;

        // Convert A2A result to MCP result format
        result
            .map(|r| r.to_mcp_result())
            .map_err(|e| ServitorError::Mcp {
                reason: format!("A2A delegation failed: {}", e),
            })
    }

    /// Publish A2A delegation event to egregore feed as a trace span.
    async fn publish_a2a_delegation(&self, task: &Task, params: A2aDelegationParams<'_>) {
        let Some(egregore) = self.egregore else {
            return;
        };

        let A2aDelegationParams {
            agent_name,
            skill_name,
            task_id,
            arguments,
            start_ts,
            end_ts,
            success,
        } = params;

        // Compute hash of input for audit trail (not the full input to avoid leaking data)
        let input_hash = {
            let mut hasher = Sha256::new();
            hasher.update(
                serde_json::to_string(arguments)
                    .unwrap_or_default()
                    .as_bytes(),
            );
            let hash = hasher.finalize();
            format!(
                "sha256:{}",
                hash.iter()
                    .take(8)
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        };

        let span_id = format!("a2a-{}-{}", agent_name, task_id);
        let status = if success {
            TraceSpanStatus::Ok
        } else {
            TraceSpanStatus::Error
        };

        let mut span = TraceSpan::new(
            task.hash.clone(),             // trace_id: use task hash for correlation
            span_id,                       // span_id: unique for this delegation
            None,                          // parent_span_id
            format!("a2a:{}", skill_name), // name
            agent_name,                    // service
            start_ts,
            end_ts,
            status,
        );

        // Add delegation-specific attributes
        span.attributes
            .insert("a2a.agent".to_string(), serde_json::json!(agent_name));
        span.attributes
            .insert("a2a.skill".to_string(), serde_json::json!(skill_name));
        span.attributes
            .insert("a2a.task_id".to_string(), serde_json::json!(task_id));
        span.attributes
            .insert("a2a.input_hash".to_string(), serde_json::json!(input_hash));

        if let Err(error) = egregore.publish_trace_span(&span).await {
            tracing::debug!(
                error = %error,
                agent = %agent_name,
                skill = %skill_name,
                "failed to publish A2A delegation span"
            );
        }
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

    async fn load_context(&self, task: &Task) -> Result<ConversationContext> {
        let mut context = ConversationContext::new();

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

        Ok(context)
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

    fn build_plan_prompt(&self, task: &Task) -> String {
        let mut prompt = self.build_system_prompt(task);
        prompt.push_str(
            "Planning mode: do not claim that the task has already been executed. \
             Produce the tool calls you intend to make in order using tool_use blocks. \
             If the task can be answered without tools, return a brief summary and no tool_use blocks.\n",
        );
        prompt
    }

    fn validate_tool_call(
        &self,
        task: &Task,
        prefixed_name: &str,
        arguments: &serde_json::Value,
        tools: &[LlmTool],
    ) -> Result<(String, String)> {
        let tool = tools
            .iter()
            .find(|tool| tool.name == prefixed_name)
            .ok_or_else(|| ServitorError::PlanValidation {
                reason: format!("unknown tool in plan: {}", prefixed_name),
            })?;

        let (mcp_name, tool_name) = parse_prefixed_tool_name(prefixed_name)?;

        if let (Some(authority), Some(keeper_name)) = (self.authority, &self.keeper_name) {
            let skill = format!("{}:{}", mcp_name, tool_name);
            let auth_result = authority.authorize_skill(keeper_name, &skill);
            if !auth_result.allowed {
                return Err(ServitorError::Unauthorized {
                    reason: format!(
                        "keeper '{}' not authorized for skill '{}': {}",
                        keeper_name, skill, auth_result.reason
                    ),
                });
            }
        }

        self.scope_enforcer
            .check(mcp_name, tool_name, arguments, task.scope_override.as_ref())
            .map_err(|error| match error {
                ServitorError::ScopeViolation { reason } => ServitorError::ScopeViolation {
                    reason: format!("task '{}' scope violation: {}", task.hash, reason),
                },
                other => other,
            })?;
        validate_json_schema(prefixed_name, &tool.input_schema, arguments)?;

        Ok((mcp_name.to_string(), tool_name.to_string()))
    }

    fn build_plan(
        &self,
        task: &Task,
        response: &crate::agent::provider::ChatResponse,
        tool_calls: Vec<PlannedToolCall>,
    ) -> Result<TaskPlan> {
        let summary = response.text();
        let stop_reason = response.stop_reason.as_str().to_string();
        let plan_hash = compute_plan_hash(task, &summary, &stop_reason, &tool_calls)?;
        let signature = self.identity.sign_hash(&plan_hash);

        Ok(TaskPlan {
            msg_type: "task_plan".to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            task_hash: task.hash.clone(),
            plan_hash,
            summary,
            stop_reason,
            tool_calls,
            attestation: Attestation {
                servitor_id: self.identity.public_id(),
                signature,
                timestamp: Utc::now(),
            },
        })
    }

    /// Build a TaskResult with signed attestation.
    fn build_result(
        &self,
        task: &Task,
        status: TaskStatus,
        result: Option<serde_json::Value>,
        error: Option<String>,
        trace_id: Option<String>,
        plan_hash: Option<&str>,
    ) -> Result<TaskResult> {
        // Compute result hash
        let result_hash = compute_result_hash(&result, &error, plan_hash, trace_id.as_deref());

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
            plan_hash: plan_hash.map(str::to_string),
            attestation,
            trace_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tool_trace_span(
        &self,
        trace_id: &str,
        parent_span_id: &str,
        service: &str,
        span_id: &str,
        mcp_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
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
        // Add sanitized arguments for forensic completeness (redacts sensitive fields)
        span.attributes.insert(
            "arguments".to_string(),
            serde_json::Value::String(sanitize_arguments(arguments)),
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
    plan_hash: Option<&str>,
    trace_id: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();

    if let Some(r) = result {
        hasher.update(serde_json::to_string(r).unwrap_or_default().as_bytes());
    }
    if let Some(e) = error {
        hasher.update(e.as_bytes());
    }
    if let Some(plan_hash) = plan_hash {
        hasher.update(plan_hash.as_bytes());
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

fn compute_plan_hash(
    task: &Task,
    summary: &str,
    stop_reason: &str,
    tool_calls: &[PlannedToolCall],
) -> Result<String> {
    let payload = serde_json::json!({
        "task_hash": task.hash,
        "summary": summary,
        "stop_reason": stop_reason,
        "tool_calls": tool_calls,
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload)?);
    let hash = hasher.finalize();
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
}

fn parse_prefixed_tool_name(prefixed_name: &str) -> Result<(&str, &str)> {
    prefixed_name
        .split_once('_')
        .ok_or_else(|| ServitorError::PlanValidation {
            reason: format!("invalid prefixed tool name: {}", prefixed_name),
        })
}

fn validate_json_schema(
    tool_name: &str,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<()> {
    validate_schema_node("$", schema, value).map_err(|reason| ServitorError::PlanValidation {
        reason: format!(
            "tool '{}' arguments failed schema validation: {}",
            tool_name, reason
        ),
    })
}

fn validate_schema_node(
    path: &str,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> std::result::Result<(), String> {
    if let Some(options) = schema.get("anyOf").and_then(|v| v.as_array()) {
        if options
            .iter()
            .any(|option| validate_schema_node(path, option, value).is_ok())
        {
            return Ok(());
        }
        return Err(format!("{} did not satisfy anyOf alternatives", path));
    }

    if let Some(options) = schema.get("oneOf").and_then(|v| v.as_array()) {
        let matches = options
            .iter()
            .filter(|option| validate_schema_node(path, option, value).is_ok())
            .count();
        if matches == 1 {
            return Ok(());
        }
        return Err(format!(
            "{} matched {} oneOf alternatives, expected exactly 1",
            path, matches
        ));
    }

    if let Some(expected) = schema.get("type").and_then(|v| v.as_str()) {
        match expected {
            "object" if !value.is_object() => {
                return Err(format!("{} expected object", path));
            }
            "array" if !value.is_array() => {
                return Err(format!("{} expected array", path));
            }
            "string" if !value.is_string() => {
                return Err(format!("{} expected string", path));
            }
            "number" if !value.is_number() => {
                return Err(format!("{} expected number", path));
            }
            "integer" if value.as_i64().is_none() && value.as_u64().is_none() => {
                return Err(format!("{} expected integer", path));
            }
            "boolean" if !value.is_boolean() => {
                return Err(format!("{} expected boolean", path));
            }
            "null" if !value.is_null() => {
                return Err(format!("{} expected null", path));
            }
            _ => {}
        }
    }

    if let Some(expected) = schema.get("const") {
        if value != expected {
            return Err(format!("{} did not match const value", path));
        }
    }

    if let Some(options) = schema.get("enum").and_then(|v| v.as_array()) {
        if !options.iter().any(|option| option == value) {
            return Err(format!("{} not in enum set", path));
        }
    }

    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        let object = value
            .as_object()
            .ok_or_else(|| format!("{} expected object for required fields", path))?;
        for field in required.iter().filter_map(|field| field.as_str()) {
            if !object.contains_key(field) {
                return Err(format!("{} missing required field '{}'", path, field));
            }
        }
    }

    if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
        let object = value
            .as_object()
            .ok_or_else(|| format!("{} expected object for properties", path))?;

        if matches!(
            schema.get("additionalProperties"),
            Some(serde_json::Value::Bool(false))
        ) {
            for key in object.keys() {
                if !properties.contains_key(key) {
                    return Err(format!("{} contains unexpected field '{}'", path, key));
                }
            }
        }

        for (key, child_schema) in properties {
            if let Some(child_value) = object.get(key) {
                let child_path = format!("{}.{}", path, key);
                validate_schema_node(&child_path, child_schema, child_value)?;
            }
        }
    }

    if let Some(items) = schema.get("items") {
        let values = value
            .as_array()
            .ok_or_else(|| format!("{} expected array for items", path))?;
        for (idx, item) in values.iter().enumerate() {
            let child_path = format!("{}[{}]", path, idx);
            validate_schema_node(&child_path, items, item)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egregore::messages::PlannedToolCall;
    use crate::identity::Identity;

    #[test]
    fn result_hash_deterministic() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None, None, None);
        let h2 = compute_result_hash(&result, &None, None, None);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn different_content_different_hash() {
        let r1 = Some(serde_json::json!({"foo": "bar"}));
        let r2 = Some(serde_json::json!({"foo": "baz"}));
        let h1 = compute_result_hash(&r1, &None, None, None);
        let h2 = compute_result_hash(&r2, &None, None, None);
        assert_ne!(h1, h2);
    }

    #[test]
    fn trace_id_changes_result_hash() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None, None, Some("trace-a"));
        let h2 = compute_result_hash(&result, &None, None, Some("trace-b"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn result_hash_changes_when_plan_hash_changes() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None, Some("plan-a"), None);
        let h2 = compute_result_hash(&result, &None, Some("plan-b"), None);
        assert_ne!(h1, h2);
    }

    #[test]
    fn parse_prefixed_tool_name_splits_server_and_tool() {
        let (server, tool) = parse_prefixed_tool_name("shell_execute").unwrap();
        assert_eq!(server, "shell");
        assert_eq!(tool, "execute");
    }

    #[test]
    fn schema_validation_rejects_missing_required_field() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string" }
            },
            "additionalProperties": false
        });

        let error = validate_json_schema("shell_execute", &schema, &serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing required field 'command'"));
    }

    #[test]
    fn schema_validation_accepts_valid_payload() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string" },
                "timeout": { "type": "integer" }
            },
            "additionalProperties": false
        });

        validate_json_schema(
            "shell_execute",
            &schema,
            &serde_json::json!({
                "command": "pwd",
                "timeout": 5
            }),
        )
        .unwrap();
    }

    #[test]
    fn plan_hash_and_attestation_are_stable() {
        let identity = Identity::generate();
        let task = Task {
            msg_type: "task".to_string(),
            id: Some("task-123".to_string()),
            hash: "task-123".to_string(),
            task_type: None,
            request: Some("List files".to_string()),
            requestor: None,
            prompt: "List files".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: std::collections::HashMap::new(),
            scope_override: None,
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
        };
        let tool_calls = vec![PlannedToolCall {
            id: "toolu_1".to_string(),
            name: "shell_execute".to_string(),
            arguments: serde_json::json!({ "command": "pwd" }),
        }];
        let hash = compute_plan_hash(&task, "Run pwd", "tool_use", &tool_calls).unwrap();
        let signature = identity.sign_hash(&hash);

        assert_eq!(hash.len(), 64);
        assert!(identity
            .public_id()
            .verify(hash.as_bytes(), &signature)
            .unwrap());
    }
}
