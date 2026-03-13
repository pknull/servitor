//! Agent loop — tool_use → execute → feed_back cycle.

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::agent::context::ConversationContext;
use crate::agent::provider::{ContentBlock, Provider, StopReason};
use crate::authority::Authority;
use crate::config::AgentConfig;
use crate::egregore::messages::{Attestation, Task, TaskResult, TaskStatus};
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
                return self.build_result(
                    task,
                    TaskStatus::Timeout,
                    None,
                    Some(format!("max turns ({}) exceeded", max_turns)),
                );
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
                return self.build_result(
                    task,
                    TaskStatus::Success,
                    Some(serde_json::json!({ "text": result_text })),
                    None,
                );
            }

            // Extract and execute tool calls
            let tool_uses = response.tool_uses();
            if tool_uses.is_empty() {
                // No tools to execute, treat as end
                let result_text = response.text();
                return self.build_result(
                    task,
                    TaskStatus::Success,
                    Some(serde_json::json!({ "text": result_text })),
                    None,
                );
            }

            // Execute each tool call
            let mut tool_results = Vec::new();
            for (tool_id, tool_name, arguments) in tool_uses {
                let result = self.execute_tool(task, tool_name, arguments).await;
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
    ) -> Result<TaskResult> {
        // Compute result hash
        let result_hash = compute_result_hash(&result, &error);

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
        })
    }
}

/// Compute SHA-256 hash of the result content.
fn compute_result_hash(result: &Option<serde_json::Value>, error: &Option<String>) -> String {
    let mut hasher = Sha256::new();

    if let Some(r) = result {
        hasher.update(serde_json::to_string(r).unwrap_or_default().as_bytes());
    }
    if let Some(e) = error {
        hasher.update(e.as_bytes());
    }

    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_hash_deterministic() {
        let result = Some(serde_json::json!({"foo": "bar"}));
        let h1 = compute_result_hash(&result, &None);
        let h2 = compute_result_hash(&result, &None);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn different_content_different_hash() {
        let r1 = Some(serde_json::json!({"foo": "bar"}));
        let r2 = Some(serde_json::json!({"foo": "baz"}));
        let h1 = compute_result_hash(&r1, &None);
        let h2 = compute_result_hash(&r2, &None);
        assert_ne!(h1, h2);
    }
}
