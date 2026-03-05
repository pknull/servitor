//! TOML schema definitions for Servitor configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root configuration structure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Identity configuration.
    #[serde(default)]
    pub identity: IdentityConfig,

    /// Egregore network configuration.
    #[serde(default)]
    pub egregore: EgregoreConfig,

    /// LLM provider configuration.
    pub llm: LlmConfig,

    /// MCP server configurations.
    #[serde(default)]
    pub mcp: HashMap<String, McpServerConfig>,

    /// Agent execution parameters.
    #[serde(default)]
    pub agent: AgentConfig,

    /// Heartbeat configuration.
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

    /// Scheduled tasks.
    #[serde(default)]
    pub schedule: Vec<ScheduledTask>,

    /// Event watchers.
    #[serde(default)]
    pub watch: Vec<WatchConfig>,
}

/// Identity storage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdentityConfig {
    /// Directory for identity storage (default: ~/.servitor).
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
        }
    }
}

fn default_data_dir() -> String {
    "~/.servitor".to_string()
}

/// Egregore network configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EgregoreConfig {
    /// Egregore API URL.
    #[serde(default = "default_egregore_url")]
    pub api_url: String,

    /// Consumer group configuration.
    #[serde(default)]
    pub group: Option<ConsumerGroupConfig>,
}

impl Default for EgregoreConfig {
    fn default() -> Self {
        Self {
            api_url: default_egregore_url(),
            group: None,
        }
    }
}

fn default_egregore_url() -> String {
    "http://127.0.0.1:7654".to_string()
}

/// Consumer group for feed partitioning.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConsumerGroupConfig {
    /// Group name.
    pub name: String,

    /// Heartbeat interval in seconds.
    #[serde(default = "default_group_heartbeat")]
    pub heartbeat_interval_secs: u64,
}

fn default_group_heartbeat() -> u64 {
    10
}

/// LLM provider configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    /// Provider type: "anthropic", "openai", "ollama", "openai-compat".
    pub provider: String,

    /// Model identifier.
    pub model: String,

    /// Environment variable containing API key.
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Base URL for OpenAI-compatible providers.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Temperature for sampling.
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// MCP server configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    /// Transport type: "stdio" or "http".
    pub transport: String,

    /// Command to execute (for stdio transport).
    #[serde(default)]
    pub command: Option<String>,

    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// HTTP URL (for http transport).
    #[serde(default)]
    pub url: Option<String>,

    /// Scope enforcement rules.
    #[serde(default)]
    pub scope: ScopeConfig,

    /// Timeout for tool calls in seconds.
    #[serde(default = "default_mcp_timeout")]
    pub timeout_secs: u64,
}

fn default_mcp_timeout() -> u64 {
    60
}

/// Scope enforcement configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ScopeConfig {
    /// Allowed patterns (glob syntax).
    #[serde(default)]
    pub allow: Vec<String>,

    /// Blocked patterns (glob syntax, takes precedence over allow).
    #[serde(default)]
    pub block: Vec<String>,
}

/// Agent execution parameters.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    /// Maximum turns (LLM round-trips) per task.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Task timeout in seconds.
    #[serde(default = "default_task_timeout")]
    pub timeout_secs: u64,

    /// System prompt prefix.
    #[serde(default)]
    pub system_prompt: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            timeout_secs: default_task_timeout(),
            system_prompt: None,
        }
    }
}

fn default_max_turns() -> u32 {
    50
}

fn default_task_timeout() -> u64 {
    300
}

/// Heartbeat emission configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HeartbeatConfig {
    /// Heartbeat interval in seconds.
    #[serde(default = "default_heartbeat_interval")]
    pub interval_secs: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_heartbeat_interval(),
        }
    }
}

fn default_heartbeat_interval() -> u64 {
    10
}

/// Scheduled task configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScheduledTask {
    /// Task name.
    pub name: String,

    /// Cron expression.
    pub cron: String,

    /// Task prompt.
    pub task: String,

    /// Notification channel.
    #[serde(default)]
    pub notify: Option<String>,
}

/// Event watcher configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfig {
    /// Watcher name.
    pub name: String,

    /// MCP server to watch.
    pub mcp: String,

    /// Event type to watch for.
    pub event: String,

    /// Filter criteria.
    #[serde(default)]
    pub filter: HashMap<String, serde_json::Value>,

    /// Task prompt (supports {{event.field}} interpolation).
    pub task: String,

    /// Notification channel.
    #[serde(default)]
    pub notify: Option<String>,
}
