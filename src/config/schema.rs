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

    /// MCP server configurations.
    #[serde(default)]
    pub mcp: HashMap<String, McpServerConfig>,

    /// A2A agent configurations.
    #[serde(default)]
    pub a2a: HashMap<String, A2aAgentConfig>,

    /// A2A server configuration (for receiving tasks from external agents).
    #[serde(default)]
    pub a2a_server: Option<A2aServerConfig>,

    /// Agent execution parameters.
    #[serde(default)]
    pub agent: AgentConfig,

    /// Network task assignment flow configuration.
    #[serde(default)]
    pub task: TaskConfig,

    /// Heartbeat configuration.
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

    /// Planner-facing executor metadata published in profile/manifest messages.
    #[serde(default)]
    pub profile: ProfileConfig,

    /// Scheduled tasks.
    #[serde(default)]
    pub schedule: Vec<ScheduledTask>,

    /// Event watchers.
    #[serde(default)]
    pub watch: Vec<WatchConfig>,

    /// Metrics configuration.
    #[serde(default)]
    pub metrics: MetricsConfig,
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

/// Structured tool call template used by schedules and notification handlers.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolCallTemplate {
    /// Prefixed MCP or A2A tool name.
    pub name: String,

    /// Arguments to pass when the task is emitted.
    #[serde(default = "default_tool_call_arguments")]
    pub arguments: serde_json::Value,
}

fn default_tool_call_arguments() -> serde_json::Value {
    serde_json::json!({})
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

    /// Structured tool calls to emit for notifications.
    #[serde(default)]
    pub on_notification: Vec<ToolCallTemplate>,
}

fn default_mcp_timeout() -> u64 {
    60
}

/// A2A agent configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct A2aAgentConfig {
    /// Base URL for agent API.
    pub url: String,

    /// URL for agent card (defaults to {url}/.well-known/agent.json).
    #[serde(default)]
    pub card: Option<String>,

    /// Timeout for task completion in seconds.
    #[serde(default = "default_a2a_timeout")]
    pub timeout_secs: u64,

    /// Poll interval for task status in milliseconds.
    #[serde(default = "default_a2a_poll_interval")]
    pub poll_interval_ms: u64,

    /// Authentication configuration.
    #[serde(default)]
    pub auth: Option<A2aAuthConfig>,

    /// Scope enforcement rules.
    #[serde(default)]
    pub scope: ScopeConfig,
}

fn default_a2a_timeout() -> u64 {
    300
}

fn default_a2a_poll_interval() -> u64 {
    2000
}

/// A2A authentication configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct A2aAuthConfig {
    /// Authentication type: "bearer" or "api_key".
    #[serde(rename = "type")]
    pub auth_type: String,

    /// Environment variable containing the token/key.
    #[serde(default)]
    pub token_env: Option<String>,

    /// Header name for API key authentication.
    #[serde(default)]
    pub header: Option<String>,
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
    /// Task timeout in seconds.
    #[serde(default = "default_task_timeout")]
    pub timeout_secs: u64,

    /// Publish detailed trace spans for task and tool execution.
    #[serde(default)]
    pub publish_trace_spans: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_task_timeout(),
            publish_trace_spans: false,
        }
    }
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

/// Planner-facing executor metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileConfig {
    /// Low-cardinality placement roles such as "staging" or "docker-host".
    #[serde(default)]
    pub roles: Vec<String>,

    /// Small stable key/value labels used for placement or filtering.
    #[serde(default)]
    pub labels: HashMap<String, String>,

    /// Operator-curated deployment targets exposed by this servitor.
    #[serde(default)]
    pub targets: Vec<DeploymentTargetConfig>,
}

/// Planner-facing deployment target summary.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeploymentTargetConfig {
    /// Stable target identifier used by planners.
    pub target_id: String,

    /// Target kind, for example "docker_compose_project".
    pub kind: String,

    /// Optional short operator-facing description.
    #[serde(default)]
    pub summary: Option<String>,

    /// Optional target-specific roles such as "web" or "staging".
    #[serde(default)]
    pub roles: Vec<String>,

    /// Snapshot freshness horizon in seconds.
    #[serde(default = "default_snapshot_ttl_secs")]
    pub snapshot_ttl_secs: u64,

    /// Optional MCP probe calls used to build environment_snapshot state.
    #[serde(default)]
    pub snapshot_tool_calls: Vec<ToolCallTemplate>,
}

fn default_snapshot_ttl_secs() -> u64 {
    120
}

/// Scheduled task configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScheduledTask {
    /// Task name.
    pub name: String,

    /// Cron expression.
    pub cron: String,

    /// Optional human-readable prompt stored with the generated task.
    #[serde(default)]
    pub prompt: Option<String>,

    /// Structured tool calls to execute when the schedule fires.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallTemplate>,

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

    /// Optional human-readable prompt stored with the generated task.
    #[serde(default)]
    pub prompt: Option<String>,

    /// Structured tool calls to execute for matching notifications.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallTemplate>,

    /// Notification channel.
    #[serde(default)]
    pub notify: Option<String>,
}

/// Prometheus metrics configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsConfig {
    /// Enable metrics endpoint.
    #[serde(default)]
    pub enabled: bool,

    /// Bind address for metrics server.
    #[serde(default = "default_metrics_bind")]
    pub bind: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_metrics_bind(),
        }
    }
}

fn default_metrics_bind() -> String {
    "127.0.0.1:9091".to_string()
}

/// A2A server configuration for receiving tasks from external agents.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct A2aServerConfig {
    /// Enable the A2A server.
    #[serde(default)]
    pub enabled: bool,

    /// Bind address (host:port).
    #[serde(default = "default_a2a_server_bind")]
    pub bind: String,

    /// Agent name (used in agent card).
    #[serde(default = "default_a2a_server_name")]
    pub name: String,

    /// Agent description (used in agent card).
    #[serde(default)]
    pub description: Option<String>,

    /// Task timeout in seconds.
    #[serde(default = "default_a2a_server_timeout")]
    pub task_timeout_secs: u64,

    /// Maximum concurrent tasks.
    #[serde(default = "default_a2a_server_max_tasks")]
    pub max_concurrent_tasks: usize,
}

impl Default for A2aServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_a2a_server_bind(),
            name: default_a2a_server_name(),
            description: None,
            task_timeout_secs: default_a2a_server_timeout(),
            max_concurrent_tasks: default_a2a_server_max_tasks(),
        }
    }
}

fn default_a2a_server_bind() -> String {
    "127.0.0.1:8765".to_string()
}

fn default_a2a_server_name() -> String {
    "servitor".to_string()
}

fn default_a2a_server_timeout() -> u64 {
    300
}

fn default_a2a_server_max_tasks() -> usize {
    10
}
