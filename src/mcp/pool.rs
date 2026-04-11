//! MCP client pool — manages multiple MCP server connections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jsonschema::JSONSchema;
use tokio::sync::RwLock;

use crate::config::{Config, McpServerConfig};
use crate::egregore::{McpServerHealth, McpServerStatus};
use crate::error::{Result, ServitorError};
use thallus_core::mcp::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use thallus_core::mcp::client::{McpClient, McpNotification, ToolDefinition};
use thallus_core::mcp::http::HttpMcpClient;
use thallus_core::mcp::stdio::StdioMcpClient;

/// Convert servitor's McpServerConfig to thallus-core's McpServerConfig.
fn to_core_config(config: &McpServerConfig) -> thallus_core::config::McpServerConfig {
    thallus_core::config::McpServerConfig {
        transport: config.transport.clone(),
        command: config.command.clone(),
        args: config.args.clone(),
        env: config.env.clone(),
        url: config.url.clone(),
        timeout_secs: config.timeout_secs,
    }
}

/// Pool of MCP clients with tool introspection.
pub struct McpPool {
    clients: HashMap<String, Arc<RwLock<Box<dyn McpClient>>>>,
    server_runtime: HashMap<String, McpServerRuntime>,
    /// Circuit breakers per server.
    circuit_breakers: HashMap<String, RwLock<CircuitBreaker>>,
    /// All tools with prefixed names, mapped to their server.
    tools: HashMap<String, RegisteredTool>,
}

struct RegisteredTool {
    server_name: String,
    definition: ToolDefinition,
    validator: Option<JSONSchema>,
}

#[derive(Debug, Clone)]
struct McpServerRuntime {
    transport: String,
    initialized: bool,
}

impl McpPool {
    /// Create a new empty pool.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            server_runtime: HashMap::new(),
            circuit_breakers: HashMap::new(),
            tools: HashMap::new(),
        }
    }

    /// Create a pool from configuration.
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut pool = Self::new();

        for (name, mcp_config) in &config.mcp {
            pool.add_client(name, mcp_config)?;
        }

        Ok(pool)
    }

    /// Add a client to the pool.
    pub fn add_client(&mut self, name: &str, config: &McpServerConfig) -> Result<()> {
        let core_config = to_core_config(config);
        let client: Box<dyn McpClient> = match config.transport.as_str() {
            "stdio" => Box::new(StdioMcpClient::new(name, core_config)),
            "http" => Box::new(HttpMcpClient::new(name, &core_config)?),
            other => {
                return Err(ServitorError::Mcp {
                    reason: format!("unknown transport: {}", other),
                })
            }
        };

        self.clients
            .insert(name.to_string(), Arc::new(RwLock::new(client)));
        self.server_runtime.insert(
            name.to_string(),
            McpServerRuntime {
                transport: config.transport.clone(),
                initialized: false,
            },
        );

        // Add circuit breaker for this server
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(30),
            success_threshold: 1,
        };
        self.circuit_breakers.insert(
            name.to_string(),
            RwLock::new(CircuitBreaker::new(cb_config)),
        );

        Ok(())
    }

    /// Initialize all clients and introspect tools.
    pub async fn initialize_all(&mut self) -> Result<()> {
        let clients: Vec<_> = self
            .clients
            .iter()
            .map(|(name, client)| (name.clone(), Arc::clone(client)))
            .collect();

        for (name, client) in clients {
            let mut client = client.write().await;

            // Initialize the server
            client.initialize().await?;

            // List and register tools
            let tools = client.list_tools().await?;
            for tool in tools {
                let prefixed_name = tool.prefixed_name(&name);
                let validator = compile_validator(&tool)?;
                self.tools.insert(
                    prefixed_name,
                    RegisteredTool {
                        server_name: name.clone(),
                        definition: tool,
                        validator,
                    },
                );
            }

            if let Some(runtime) = self.server_runtime.get_mut(&name) {
                runtime.initialized = true;
            }
        }

        tracing::info!(
            servers = self.clients.len(),
            tools = self.tools.len(),
            "initialized MCP pool"
        );

        Ok(())
    }

    /// Get all available tools with prefixed names.
    pub fn all_tools(&self) -> Vec<(&str, &ToolDefinition)> {
        self.tools
            .iter()
            .map(|(prefixed, tool)| (prefixed.as_str(), &tool.definition))
            .collect()
    }

    /// Get tools formatted for LLM consumption.
    pub fn tools_for_llm(&self) -> Vec<LlmTool> {
        self.tools
            .iter()
            .map(|(prefixed_name, tool)| LlmTool {
                name: prefixed_name.clone(),
                description: tool.definition.description.clone(),
                input_schema: tool.definition.input_schema.clone().unwrap_or_else(|| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {}
                    })
                }),
            })
            .collect()
    }

    /// Parse a prefixed tool name into (server_name, tool_name).
    pub fn parse_tool_name<'a>(&'a self, prefixed: &'a str) -> Option<(&'a str, &'a str)> {
        if let Some(tool) = self.tools.get(prefixed) {
            // Extract the original tool name by removing the prefix
            let prefix_len = tool.server_name.len() + 1; // server_name + underscore
            if prefixed.len() > prefix_len {
                let tool_name = &prefixed[prefix_len..];
                return Some((tool.server_name.as_str(), tool_name));
            }
        }
        None
    }

    /// Call a tool by its prefixed name.
    ///
    /// Respects circuit breaker state — rejects calls if the server's
    /// circuit is open (too many recent failures).
    pub async fn call_tool(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<crate::mcp::client::ToolCallResult> {
        let tool = self
            .tools
            .get(prefixed_name)
            .ok_or_else(|| ServitorError::Mcp {
                reason: format!("unknown tool: {}", prefixed_name),
            })?;
        let tool_name = tool.definition.name.as_str();
        let server_name = &tool.server_name;

        // Check circuit breaker
        if let Some(cb) = self.circuit_breakers.get(server_name) {
            let mut cb = cb.write().await;
            if !cb.should_allow() {
                tracing::warn!(
                    server = %server_name,
                    tool = %prefixed_name,
                    "circuit breaker open, rejecting call"
                );
                return Err(ServitorError::Mcp {
                    reason: format!(
                        "circuit breaker open for server '{}' — too many failures",
                        server_name
                    ),
                });
            }
        }

        validate_arguments(prefixed_name, tool, &arguments)?;

        let client =
            self.clients
                .get(server_name)
                .ok_or_else(|| ServitorError::McpServerNotFound {
                    name: server_name.to_string(),
                })?;

        let client = client.read().await;
        let result = client.call_tool(tool_name, arguments).await;

        // Record success/failure in circuit breaker
        if let Some(cb) = self.circuit_breakers.get(server_name) {
            let mut cb = cb.write().await;
            match &result {
                Ok(_) => cb.record_success(),
                Err(_) => cb.record_failure(),
            }
        }

        result.map_err(ServitorError::from)
    }

    /// Get capability classes (server names).
    pub fn capabilities(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Drain pending notifications observed from MCP servers.
    pub async fn drain_notifications(&self) -> Result<Vec<(String, McpNotification)>> {
        let mut notifications = Vec::new();

        for (name, client) in &self.clients {
            let client = client.read().await;
            let server_notifications = client.drain_notifications().await?;
            notifications.extend(
                server_notifications
                    .into_iter()
                    .map(|notification| (name.clone(), notification)),
            );
        }

        Ok(notifications)
    }

    /// Get a health snapshot for each configured MCP server.
    ///
    /// Considers both ping results and circuit breaker state.
    pub async fn server_statuses(&self) -> Vec<McpServerStatus> {
        let mut statuses = Vec::with_capacity(self.clients.len());

        for (name, client) in &self.clients {
            let Some(runtime) = self.server_runtime.get(name) else {
                continue;
            };

            // Check circuit breaker state first
            let circuit_open = if let Some(cb) = self.circuit_breakers.get(name) {
                let cb = cb.read().await;
                cb.state() == CircuitState::Open
            } else {
                false
            };

            let status = if !runtime.initialized {
                McpServerHealth::Unavailable
            } else if circuit_open {
                // Circuit is open — report as degraded without pinging
                McpServerHealth::Degraded
            } else {
                let client = client.read().await;
                match client.ping().await {
                    Ok(()) => McpServerHealth::Healthy,
                    Err(error) => {
                        tracing::debug!(name = %name, error = %error, "MCP server ping failed");
                        McpServerHealth::Degraded
                    }
                }
            };

            statuses.push(McpServerStatus {
                name: name.clone(),
                transport: runtime.transport.clone(),
                status,
            });
        }

        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        statuses
    }

    /// Run health checks on all servers and update circuit breakers.
    ///
    /// Call this periodically to proactively detect server failures.
    pub async fn health_check(&self) {
        for (name, client) in &self.clients {
            let Some(runtime) = self.server_runtime.get(name) else {
                continue;
            };

            if !runtime.initialized {
                continue;
            }

            let client = client.read().await;
            let result = client.ping().await;

            if let Some(cb) = self.circuit_breakers.get(name) {
                let mut cb = cb.write().await;
                match result {
                    Ok(()) => {
                        // Only record success if circuit was half-open (testing recovery)
                        if cb.state() == CircuitState::HalfOpen {
                            cb.record_success();
                            tracing::info!(server = %name, "MCP server recovered");
                        }
                    }
                    Err(error) => {
                        cb.record_failure();
                        tracing::warn!(server = %name, error = %error, "MCP server health check failed");
                    }
                }
            }
        }
    }

    /// Get circuit breaker state for a server.
    pub async fn circuit_state(&self, server_name: &str) -> Option<CircuitState> {
        if let Some(cb) = self.circuit_breakers.get(server_name) {
            let cb = cb.read().await;
            Some(cb.state())
        } else {
            None
        }
    }

    /// Manually reset a server's circuit breaker.
    pub async fn reset_circuit(&self, server_name: &str) {
        if let Some(cb) = self.circuit_breakers.get(server_name) {
            let mut cb = cb.write().await;
            cb.reset();
            tracing::info!(server = %server_name, "circuit breaker manually reset");
        }
    }

    /// Shutdown all clients.
    pub async fn shutdown_all(&self) -> Result<()> {
        for client in self.clients.values() {
            let mut client = client.write().await;
            let _ = client.shutdown().await;
        }
        Ok(())
    }
}

impl Default for McpPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool definition formatted for LLM consumption -- re-exported from thallus-core.
pub use thallus_core::mcp::pool::LlmTool;

fn compile_validator(tool: &ToolDefinition) -> Result<Option<JSONSchema>> {
    let Some(schema) = tool.input_schema.as_ref() else {
        return Ok(None);
    };

    JSONSchema::options()
        .compile(schema)
        .map(Some)
        .map_err(|error| ServitorError::Mcp {
            reason: format!("invalid input schema for tool '{}': {}", tool.name, error),
        })
}

fn validate_arguments(
    prefixed_name: &str,
    tool: &RegisteredTool,
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

    tracing::warn!(tool = prefixed_name, reason = %details, "rejected MCP tool call");
    Err(ServitorError::McpValidation {
        tool: prefixed_name.to_string(),
        reason: details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use thallus_core::mcp::client::{
        InitializeResult, McpNotification, ServerCapabilities, ServerInfo, ToolCallResult,
    };

    struct FakeClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl McpClient for FakeClient {
        async fn initialize(&mut self) -> thallus_core::Result<InitializeResult> {
            Ok(InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                server_info: ServerInfo {
                    name: "fake".to_string(),
                    version: "1.0.0".to_string(),
                },
                capabilities: ServerCapabilities::default(),
            })
        }

        async fn list_tools(&self) -> thallus_core::Result<Vec<ToolDefinition>> {
            Ok(vec![])
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> thallus_core::Result<ToolCallResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ok"))
        }

        async fn ping(&self) -> thallus_core::Result<()> {
            Ok(())
        }

        async fn drain_notifications(&self) -> thallus_core::Result<Vec<McpNotification>> {
            Ok(vec![])
        }

        async fn shutdown(&mut self) -> thallus_core::Result<()> {
            Ok(())
        }

        fn name(&self) -> &str {
            "fake"
        }
    }

    fn pool_with_tool(schema: serde_json::Value) -> (McpPool, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = McpPool::new();
        pool.clients.insert(
            "shell".to_string(),
            Arc::new(RwLock::new(Box::new(FakeClient {
                calls: calls.clone(),
            }))),
        );

        // Add circuit breaker for the server
        pool.circuit_breakers
            .insert("shell".to_string(), RwLock::new(CircuitBreaker::default()));

        let definition = ToolDefinition {
            name: "execute".to_string(),
            description: Some("Execute a shell command".to_string()),
            input_schema: Some(schema),
        };
        let validator = compile_validator(&definition).unwrap();
        pool.tools.insert(
            "shell_execute".to_string(),
            RegisteredTool {
                server_name: "shell".to_string(),
                definition,
                validator,
            },
        );

        (pool, calls)
    }

    #[tokio::test]
    async fn rejects_invalid_arguments_before_transport_call() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"],
            "additionalProperties": false
        });
        let (pool, calls) = pool_with_tool(schema);

        let error = pool
            .call_tool("shell_execute", serde_json::json!({ "command": 42 }))
            .await
            .unwrap_err();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(error, ServitorError::McpValidation { .. }));
        assert!(error.to_string().contains("command"));
    }

    #[tokio::test]
    async fn allows_valid_arguments_through_to_transport() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"],
            "additionalProperties": false
        });
        let (pool, calls) = pool_with_tool(schema);

        let result = pool
            .call_tool("shell_execute", serde_json::json!({ "command": "ls /tmp" }))
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.text_content(), "ok");
    }

    #[tokio::test]
    async fn configured_servers_report_unavailable_before_initialization() {
        let config = Config::from_str(
            r#"
[mcp.shell]
transport = "stdio"
command = "nonexistent-mcp-server"
"#,
        )
        .unwrap();

        let pool = McpPool::from_config(&config).unwrap();
        let statuses = pool.server_statuses().await;

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].name, "shell");
        assert_eq!(statuses[0].transport, "stdio");
        assert_eq!(statuses[0].status, McpServerHealth::Unavailable);
    }

    #[tokio::test]
    async fn circuit_breaker_rejects_after_failures() {
        use crate::mcp::circuit_breaker::CircuitBreakerConfig;

        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = McpPool::new();
        pool.clients.insert(
            "failing".to_string(),
            Arc::new(RwLock::new(Box::new(FakeClient {
                calls: calls.clone(),
            }))),
        );

        // Circuit breaker that opens after 2 failures
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: std::time::Duration::from_secs(60),
            success_threshold: 1,
        };
        pool.circuit_breakers.insert(
            "failing".to_string(),
            RwLock::new(CircuitBreaker::new(cb_config)),
        );

        let definition = ToolDefinition {
            name: "test".to_string(),
            description: None,
            input_schema: None,
        };
        pool.tools.insert(
            "failing_test".to_string(),
            RegisteredTool {
                server_name: "failing".to_string(),
                definition,
                validator: None,
            },
        );

        // Manually trip the circuit breaker
        {
            let mut cb = pool.circuit_breakers.get("failing").unwrap().write().await;
            cb.record_failure();
            cb.record_failure();
            assert_eq!(cb.state(), CircuitState::Open);
        }

        // Call should be rejected
        let result = pool.call_tool("failing_test", serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("circuit breaker open"));

        // The underlying client should not have been called
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn circuit_state_query() {
        let pool = McpPool::new();

        // Non-existent server returns None
        assert!(pool.circuit_state("nonexistent").await.is_none());
    }
}
