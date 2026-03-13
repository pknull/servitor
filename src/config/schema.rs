//! TOML schema definitions for Servitor configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Deployment mode for Servitor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentMode {
    /// Server mode: egregore-connected, no local storage (current behavior).
    #[default]
    Server,
    /// Personal mode: local storage, works offline, syncs when online.
    Personal,
    /// Hybrid mode: local storage + egregore, best of both.
    Hybrid,
}

/// Root configuration structure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Identity configuration.
    #[serde(default)]
    pub identity: IdentityConfig,

    /// Deployment mode.
    #[serde(default)]
    pub deployment: DeploymentConfig,

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

    /// Memory configuration (for personal/hybrid modes).
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Sync configuration (for personal/hybrid modes).
    #[serde(default)]
    pub sync: SyncConfig,

    /// Communication transports.
    #[serde(default)]
    pub comms: CommsConfig,

    /// Scheduled tasks.
    #[serde(default)]
    pub schedule: Vec<ScheduledTask>,

    /// Event watchers.
    #[serde(default)]
    pub watch: Vec<WatchConfig>,
}

/// Deployment configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeploymentConfig {
    /// Deployment mode: server, personal, or hybrid.
    #[serde(default)]
    pub mode: DeploymentMode,
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            mode: DeploymentMode::Server,
        }
    }
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

/// Memory storage configuration (for personal/hybrid modes).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    /// Path to SQLite database file.
    #[serde(default = "default_database_path")]
    pub database: String,

    /// Cache TTL in hours for conversation history.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_hours: u64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            database: default_database_path(),
            cache_ttl_hours: default_cache_ttl(),
        }
    }
}

fn default_database_path() -> String {
    "~/.servitor/memory.db".to_string()
}

fn default_cache_ttl() -> u64 {
    168 // 1 week
}

/// Sync configuration (for personal/hybrid modes).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    /// Sync interval in seconds.
    #[serde(default = "default_sync_interval")]
    pub interval_secs: u64,

    /// Maximum results to sync per batch.
    #[serde(default = "default_sync_batch")]
    pub batch_size: usize,

    /// Enable automatic sync.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_sync_interval(),
            batch_size: default_sync_batch(),
            enabled: true,
        }
    }
}

fn default_sync_interval() -> u64 {
    60
}

fn default_sync_batch() -> usize {
    10
}

/// Egregore network configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EgregoreConfig {
    /// Egregore API URL.
    #[serde(default = "default_egregore_url")]
    pub api_url: String,

    /// Enable SSE subscription in daemon mode.
    #[serde(default)]
    pub subscribe: bool,

    /// Consumer group configuration.
    #[serde(default)]
    pub group: Option<ConsumerGroupConfig>,
}

impl Default for EgregoreConfig {
    fn default() -> Self {
        Self {
            api_url: default_egregore_url(),
            subscribe: false,
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
    /// Provider type: "anthropic", "openai", "ollama", "openai-compat", "codex".
    pub provider: String,

    /// Model identifier.
    pub model: String,

    /// Environment variable containing API key.
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Base URL for OpenAI-compatible providers.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Path to OAuth token file (for codex provider).
    /// Supports OpenClaw's auth-profiles.json format.
    #[serde(default)]
    pub token_file: Option<String>,

    /// OAuth profile name to use (default: "openai-codex:default").
    #[serde(default)]
    pub oauth_profile: Option<String>,

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

    /// Task template for notifications (supports {{notification}} interpolation).
    #[serde(default)]
    pub on_notification: Option<String>,
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

    /// Whether to publish result to egregore.
    #[serde(default)]
    pub publish: bool,

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

/// Communication transport configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CommsConfig {
    /// Discord transport.
    #[serde(default)]
    pub discord: Option<DiscordConfig>,

    /// HTTP webhook transport.
    #[serde(default)]
    pub http: Option<HttpCommsConfig>,
}

/// Discord transport configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordConfig {
    /// Environment variable containing bot token.
    pub token_env: String,

    /// Guild IDs to allow (empty = all guilds).
    #[serde(default)]
    pub guild_allowlist: Vec<String>,

    /// Require @mention to respond.
    #[serde(default = "default_true")]
    pub require_mention: bool,

    /// Channel for sending notifications.
    #[serde(default)]
    pub notification_channel: Option<String>,
}

fn default_true() -> bool {
    true
}

/// HTTP webhook transport configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpCommsConfig {
    /// Bind address.
    #[serde(default = "default_http_bind")]
    pub bind: String,

    /// Port.
    #[serde(default = "default_http_port")]
    pub port: u16,

    /// Authentication token (optional).
    #[serde(default)]
    pub auth_token: Option<String>,
}

fn default_http_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_http_port() -> u16 {
    8765
}
