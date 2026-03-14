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

    /// Network task assignment flow configuration.
    #[serde(default)]
    pub task: TaskConfig,

    /// Heartbeat configuration.
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

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

    /// Enable SSE subscription in daemon mode.
    #[serde(default)]
    pub subscribe: bool,

    /// Reserved consumer group configuration.
    ///
    /// Parsed from config but not yet wired into the runtime on this branch.
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

/// Reserved consumer group for future feed partitioning.
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
    /// Provider type: "anthropic", "openai", "ollama", "openai-compat", "codex", "claude-code".
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

    /// Publish detailed trace spans for task and tool execution.
    #[serde(default)]
    pub publish_trace_spans: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            timeout_secs: default_task_timeout(),
            system_prompt: None,
            publish_trace_spans: false,
        }
    }
}

fn default_max_turns() -> u32 {
    50
}

fn default_task_timeout() -> u64 {
    300
}

/// Task lifecycle configuration for offer/assign/execute flow.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskConfig {
    /// Offer time-to-live in seconds before withdrawal.
    #[serde(default = "default_offer_ttl_secs")]
    pub offer_ttl_secs: u64,

    /// How long a requestor has to see offers before the offer window is stale.
    #[serde(default = "default_offer_timeout_secs")]
    pub offer_timeout_secs: u64,

    /// How long an assignment remains valid before it is considered stale.
    #[serde(default = "default_assign_timeout_secs")]
    pub assign_timeout_secs: u64,

    /// How long the servitor has to acknowledge start.
    #[serde(default = "default_start_timeout_secs")]
    pub start_timeout_secs: u64,

    /// Multiplier applied to ETA when deciding a task has overrun.
    #[serde(default = "default_eta_buffer_multiplier")]
    pub eta_buffer_multiplier: f64,

    /// How long to wait after a ping before treating the task as unresponsive.
    #[serde(default = "default_ping_timeout_secs")]
    pub ping_timeout_secs: u64,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            offer_ttl_secs: default_offer_ttl_secs(),
            offer_timeout_secs: default_offer_timeout_secs(),
            assign_timeout_secs: default_assign_timeout_secs(),
            start_timeout_secs: default_start_timeout_secs(),
            eta_buffer_multiplier: default_eta_buffer_multiplier(),
            ping_timeout_secs: default_ping_timeout_secs(),
        }
    }
}

fn default_offer_ttl_secs() -> u64 {
    300
}

fn default_offer_timeout_secs() -> u64 {
    60
}

fn default_assign_timeout_secs() -> u64 {
    300
}

fn default_start_timeout_secs() -> u64 {
    30
}

fn default_eta_buffer_multiplier() -> f64 {
    1.5
}

fn default_ping_timeout_secs() -> u64 {
    30
}

/// Heartbeat emission configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HeartbeatConfig {
    /// Heartbeat interval in seconds.
    #[serde(default = "default_heartbeat_interval")]
    pub interval_secs: u64,

    /// Include enhanced runtime monitoring fields in published profiles.
    #[serde(default)]
    pub include_runtime_monitoring: bool,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_heartbeat_interval(),
            include_runtime_monitoring: false,
        }
    }
}

fn default_heartbeat_interval() -> u64 {
    300
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

    /// Reserved HTTP webhook transport.
    ///
    /// Parsed from config but not instantiated by the current runtime.
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

/// Reserved HTTP webhook transport configuration.
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
