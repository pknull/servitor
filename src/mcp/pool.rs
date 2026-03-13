//! MCP client pool — manages multiple MCP server connections.

use std::collections::HashMap;
use std::sync::Arc;

use jsonschema::JSONSchema;
use tokio::sync::RwLock;

use crate::config::{Config, McpServerConfig};
use crate::egregore::{McpServerHealth, McpServerStatus};
use crate::error::{Result, ServitorError};
use crate::mcp::client::{McpClient, ToolDefinition};
use crate::mcp::http::HttpMcpClient;
use crate::mcp::stdio::StdioMcpClient;

/// Pool of MCP clients with tool introspection.
pub struct McpPool {
    clients: HashMap<String, Arc<RwLock<Box<dyn McpClient>>>>,
    server_runtime: HashMap<String, McpServerRuntime>,
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
        let client: Box<dyn McpClient> = match config.transport.as_str() {
            "stdio" => Box::new(StdioMcpClient::new(name, config.clone())),
            "http" => Box::new(HttpMcpClient::new(name, config)?),
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

        validate_arguments(prefixed_name, tool, &arguments)?;

        let client = self.clients.get(&tool.server_name).ok_or_else(|| {
            ServitorError::McpServerNotFound {
                name: tool.server_name.to_string(),
            }
        })?;

        let client = client.read().await;
        client.call_tool(tool_name, arguments).await
    }

    /// Get capability classes (server names).
    pub fn capabilities(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Get a health snapshot for each configured MCP server.
    pub async fn server_statuses(&self) -> Vec<McpServerStatus> {
        let mut statuses = Vec::with_capacity(self.clients.len());

        for (name, client) in &self.clients {
            let Some(runtime) = self.server_runtime.get(name) else {
                continue;
            };

            let status = if !runtime.initialized {
                McpServerHealth::Unavailable
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

/// Tool definition formatted for LLM consumption.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

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

    use crate::mcp::client::{InitializeResult, ServerCapabilities, ServerInfo, ToolCallResult};

    struct FakeClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl McpClient for FakeClient {
        async fn initialize(&mut self) -> Result<InitializeResult> {
            Ok(InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                server_info: ServerInfo {
                    name: "fake".to_string(),
                    version: "1.0.0".to_string(),
                },
                capabilities: ServerCapabilities::default(),
            })
        }

        async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
            Ok(vec![])
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<ToolCallResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ok"))
        }

        async fn ping(&self) -> Result<()> {
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<()> {
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
[llm]
provider = "ollama"
model = "llama3.3:70b"

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
}
