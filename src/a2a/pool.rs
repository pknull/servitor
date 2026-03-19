//! A2A agent pool — manages multiple A2A agent connections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jsonschema::JSONSchema;
use tokio::sync::RwLock;

use super::card::{AgentCard, Skill};
use super::client::{A2aClient, TaskResult};
use super::http::{HttpA2aClient, HttpA2aConfig};
use super::{A2aError, Result};
use crate::mcp::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use crate::mcp::pool::LlmTool;

/// Pool of A2A clients with tool introspection.
pub struct A2aPool {
    clients: HashMap<String, Arc<RwLock<Box<dyn A2aClient>>>>,
    agent_runtime: HashMap<String, A2aAgentRuntime>,
    /// Circuit breakers per agent.
    circuit_breakers: HashMap<String, RwLock<CircuitBreaker>>,
    /// All skills as tools with prefixed names, mapped to their agent.
    tools: HashMap<String, RegisteredA2aTool>,
}

struct RegisteredA2aTool {
    agent_name: String,
    skill: Skill,
    validator: Option<JSONSchema>,
}

#[derive(Debug, Clone)]
struct A2aAgentRuntime {
    initialized: bool,
}

/// Configuration for adding an A2A agent to the pool.
#[derive(Debug, Clone)]
pub struct A2aAgentPoolConfig {
    /// Agent name (for tool prefixing).
    pub name: String,
    /// Base URL for agent API.
    pub url: String,
    /// URL for agent card.
    pub card_url: Option<String>,
    /// Bearer token for authentication.
    pub bearer_token: Option<String>,
    /// Timeout in seconds for task completion.
    pub timeout_secs: u64,
    /// Poll interval in milliseconds.
    pub poll_interval_ms: u64,
    /// Allowed skills (empty = all).
    pub allow_skills: Vec<String>,
}

impl A2aPool {
    /// Create a new empty pool.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            agent_runtime: HashMap::new(),
            circuit_breakers: HashMap::new(),
            tools: HashMap::new(),
        }
    }

    /// Create a pool from configuration.
    pub fn from_config(config: &crate::config::Config) -> Result<Self> {
        let mut pool = Self::new();

        for (name, a2a_config) in &config.a2a {
            // Resolve bearer token from environment
            let bearer_token = a2a_config.auth.as_ref().and_then(|auth| {
                if auth.auth_type == "bearer" {
                    auth.token_env
                        .as_ref()
                        .and_then(|env_var| std::env::var(env_var).ok())
                } else {
                    None
                }
            });

            pool.add_agent(A2aAgentPoolConfig {
                name: name.clone(),
                url: a2a_config.url.clone(),
                card_url: a2a_config.card.clone(),
                bearer_token,
                timeout_secs: a2a_config.timeout_secs,
                poll_interval_ms: a2a_config.poll_interval_ms,
                allow_skills: a2a_config.scope.allow.clone(),
            })?;
        }

        Ok(pool)
    }

    /// Add an agent to the pool.
    pub fn add_agent(&mut self, config: A2aAgentPoolConfig) -> Result<()> {
        let client = HttpA2aClient::new(HttpA2aConfig {
            name: config.name.clone(),
            url: config.url,
            card_url: config.card_url,
            bearer_token: config.bearer_token,
            timeout_secs: config.timeout_secs,
            poll_interval_ms: config.poll_interval_ms,
        })?;

        self.clients.insert(
            config.name.clone(),
            Arc::new(RwLock::new(Box::new(client) as Box<dyn A2aClient>)),
        );

        self.agent_runtime
            .insert(config.name.clone(), A2aAgentRuntime { initialized: false });

        // Add circuit breaker for this agent
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
            success_threshold: 1,
        };
        self.circuit_breakers
            .insert(config.name, RwLock::new(CircuitBreaker::new(cb_config)));

        Ok(())
    }

    /// Initialize all agents and introspect skills.
    pub async fn initialize_all(&mut self) -> Result<()> {
        let agents: Vec<_> = self
            .clients
            .iter()
            .map(|(name, client)| (name.clone(), Arc::clone(client)))
            .collect();

        for (name, client) in agents {
            let mut client = client.write().await;

            // Fetch agent card
            let card = match client.fetch_card().await {
                Ok(card) => card,
                Err(e) => {
                    tracing::warn!(agent = %name, error = %e, "failed to fetch agent card");
                    continue;
                }
            };

            // Register skills as tools
            for skill in &card.skills {
                let prefixed_name = skill.prefixed_name(&name);
                let validator = compile_skill_validator(skill)?;

                self.tools.insert(
                    prefixed_name,
                    RegisteredA2aTool {
                        agent_name: name.clone(),
                        skill: skill.clone(),
                        validator,
                    },
                );
            }

            if let Some(runtime) = self.agent_runtime.get_mut(&name) {
                runtime.initialized = true;
            }
        }

        tracing::info!(
            agents = self.clients.len(),
            tools = self.tools.len(),
            "initialized A2A pool"
        );

        Ok(())
    }

    /// Get tools formatted for LLM consumption.
    pub fn tools_for_llm(&self) -> Vec<LlmTool> {
        self.tools
            .iter()
            .map(|(prefixed_name, tool)| LlmTool {
                name: prefixed_name.clone(),
                description: tool.skill.description.clone(),
                input_schema: tool.skill.input_schema.clone().unwrap_or_else(|| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "input": {
                                "type": "string",
                                "description": "Input for the skill"
                            }
                        }
                    })
                }),
            })
            .collect()
    }

    /// Parse a prefixed tool name into (agent_name, skill_name).
    pub fn parse_tool_name<'a>(&'a self, prefixed: &'a str) -> Option<(&'a str, &'a str)> {
        if let Some(tool) = self.tools.get(prefixed) {
            let prefix_len = tool.agent_name.len() + 1; // agent_name + underscore
            if prefixed.len() > prefix_len {
                let skill_name = &prefixed[prefix_len..];
                return Some((tool.agent_name.as_str(), skill_name));
            }
        }
        None
    }

    /// Check if a tool name belongs to the A2A pool.
    pub fn has_tool(&self, prefixed_name: &str) -> bool {
        self.tools.contains_key(prefixed_name)
    }

    /// Execute a skill by its prefixed name.
    pub async fn execute_skill(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<TaskResult> {
        let tool = self
            .tools
            .get(prefixed_name)
            .ok_or_else(|| A2aError::AgentNotFound {
                name: prefixed_name.to_string(),
            })?;

        let skill_name = &tool.skill.name;
        let agent_name = &tool.agent_name;

        // Check circuit breaker
        if let Some(cb) = self.circuit_breakers.get(agent_name) {
            let mut cb = cb.write().await;
            if !cb.should_allow() {
                tracing::warn!(
                    agent = %agent_name,
                    skill = %prefixed_name,
                    "circuit breaker open, rejecting A2A call"
                );
                return Err(A2aError::Protocol {
                    reason: format!(
                        "circuit breaker open for agent '{}' — too many failures",
                        agent_name
                    ),
                });
            }
        }

        // Validate arguments
        validate_skill_arguments(prefixed_name, tool, &arguments)?;

        let client = self
            .clients
            .get(agent_name)
            .ok_or_else(|| A2aError::AgentNotFound {
                name: agent_name.to_string(),
            })?;

        let client = client.read().await;
        let result = client.execute_task(skill_name, arguments).await;

        // Record success/failure in circuit breaker
        if let Some(cb) = self.circuit_breakers.get(agent_name) {
            let mut cb = cb.write().await;
            match &result {
                Ok(_) => cb.record_success(),
                Err(_) => cb.record_failure(),
            }
        }

        result
    }

    /// Get agent names.
    pub fn agents(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Get circuit breaker state for an agent.
    pub async fn circuit_state(&self, agent_name: &str) -> Option<CircuitState> {
        if let Some(cb) = self.circuit_breakers.get(agent_name) {
            let cb = cb.read().await;
            Some(cb.state())
        } else {
            None
        }
    }

    /// Get the cached agent card for an agent.
    pub async fn agent_card(&self, agent_name: &str) -> Option<AgentCard> {
        if let Some(client) = self.clients.get(agent_name) {
            let client = client.read().await;
            client.card().cloned()
        } else {
            None
        }
    }

    /// Check if an agent is initialized.
    pub fn is_initialized(&self, agent_name: &str) -> bool {
        self.agent_runtime
            .get(agent_name)
            .map(|r| r.initialized)
            .unwrap_or(false)
    }

    /// Check if the pool has any agents.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

impl Default for A2aPool {
    fn default() -> Self {
        Self::new()
    }
}

fn compile_skill_validator(skill: &Skill) -> Result<Option<JSONSchema>> {
    let Some(schema) = skill.input_schema.as_ref() else {
        return Ok(None);
    };

    JSONSchema::options()
        .compile(schema)
        .map(Some)
        .map_err(|error| A2aError::Protocol {
            reason: format!("invalid input schema for skill '{}': {}", skill.name, error),
        })
}

fn validate_skill_arguments(
    prefixed_name: &str,
    tool: &RegisteredA2aTool,
    arguments: &serde_json::Value,
) -> Result<()> {
    let Some(validator) = tool.validator.as_ref() else {
        return Ok(());
    };

    let details = match validator.validate(arguments) {
        Ok(()) => return Ok(()),
        Err(errors) => errors
            .take(5)
            .map(|error| {
                let path = error.instance_path.to_string();
                if path.is_empty() {
                    error.to_string()
                } else {
                    format!("{}: {}", path, error)
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
    };

    tracing::warn!(skill = prefixed_name, reason = %details, "rejected A2A skill call");
    Err(A2aError::Protocol {
        reason: format!("invalid arguments for '{}': {}", prefixed_name, details),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_name_extracts_components() {
        let mut pool = A2aPool::new();
        pool.tools.insert(
            "researcher_web_search".to_string(),
            RegisteredA2aTool {
                agent_name: "researcher".to_string(),
                skill: Skill {
                    name: "web_search".to_string(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    input_modes: vec![],
                    output_modes: vec![],
                    tags: vec![],
                },
                validator: None,
            },
        );

        let result = pool.parse_tool_name("researcher_web_search");
        assert_eq!(result, Some(("researcher", "web_search")));

        assert!(pool.parse_tool_name("nonexistent_tool").is_none());
    }
}
