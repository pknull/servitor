//! MCP client pool — manages multiple MCP server connections.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::{Config, McpServerConfig};
use crate::error::{Result, ServitorError};
use crate::mcp::client::{McpClient, ToolDefinition};
use crate::mcp::http::HttpMcpClient;
use crate::mcp::stdio::StdioMcpClient;

/// Pool of MCP clients with tool introspection.
pub struct McpPool {
    clients: HashMap<String, Arc<RwLock<Box<dyn McpClient>>>>,
    /// All tools with prefixed names, mapped to their server.
    tools: HashMap<String, (String, ToolDefinition)>,
}

impl McpPool {
    /// Create a new empty pool.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
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

        self.clients.insert(name.to_string(), Arc::new(RwLock::new(client)));
        Ok(())
    }

    /// Initialize all clients and introspect tools.
    pub async fn initialize_all(&mut self) -> Result<()> {
        for (name, client) in &self.clients {
            let mut client = client.write().await;

            // Initialize the server
            client.initialize().await?;

            // List and register tools
            let tools = client.list_tools().await?;
            for tool in tools {
                let prefixed_name = tool.prefixed_name(name);
                self.tools.insert(prefixed_name, (name.clone(), tool));
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
            .map(|(prefixed, (_, tool))| (prefixed.as_str(), tool))
            .collect()
    }

    /// Get tools formatted for LLM consumption.
    pub fn tools_for_llm(&self) -> Vec<LlmTool> {
        self.tools
            .iter()
            .map(|(prefixed_name, (_, tool))| LlmTool {
                name: prefixed_name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone().unwrap_or_else(|| {
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
        if let Some((server_name, _)) = self.tools.get(prefixed) {
            // Extract the original tool name by removing the prefix
            let prefix_len = server_name.len() + 1; // server_name + underscore
            if prefixed.len() > prefix_len {
                let tool_name = &prefixed[prefix_len..];
                return Some((server_name.as_str(), tool_name));
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
        let (server_name, tool_name) = self.parse_tool_name(prefixed_name).ok_or_else(|| {
            ServitorError::Mcp {
                reason: format!("unknown tool: {}", prefixed_name),
            }
        })?;

        let client = self.clients.get(server_name).ok_or_else(|| ServitorError::McpServerNotFound {
            name: server_name.to_string(),
        })?;

        let client = client.read().await;
        client.call_tool(tool_name, arguments).await
    }

    /// Get capability classes (server names).
    pub fn capabilities(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
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
