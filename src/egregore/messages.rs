//! Egregore message schemas for Servitor.
//!
//! These are the content types that Servitor publishes to and receives from
//! the egregore network.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::identity::PublicId;

/// Servitor capability profile, published on startup and heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServitorProfile {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Servitor's public identity.
    pub servitor_id: PublicId,

    /// Capability classes available.
    pub capabilities: Vec<String>,

    /// Tool names available (prefixed with MCP server name).
    pub tools: Vec<String>,

    /// Scope constraints per capability.
    pub scopes: HashMap<String, ScopeConstraints>,

    /// Resource limits.
    #[serde(default)]
    pub resource_limits: ResourceLimits,

    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,

    /// Profile version.
    #[serde(default = "default_version")]
    pub version: String,

    /// Time since daemon start.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub uptime_secs: u64,

    /// MCP server health snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerStatus>,

    /// Current work queue and execution load.
    #[serde(default, skip_serializing_if = "ServitorLoad::is_empty")]
    pub load: ServitorLoad,

    /// Cumulative task counters.
    #[serde(default, skip_serializing_if = "ServitorStats::is_empty")]
    pub stats: ServitorStats,

    /// Timestamp when the last task finished execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_ts: Option<DateTime<Utc>>,
}

fn default_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

impl ServitorProfile {
    pub fn new(servitor_id: PublicId, heartbeat_interval_ms: u64) -> Self {
        Self {
            msg_type: "servitor_profile".to_string(),
            servitor_id,
            capabilities: vec![],
            tools: vec![],
            scopes: HashMap::new(),
            resource_limits: ResourceLimits::default(),
            heartbeat_interval_ms,
            version: default_version(),
            uptime_secs: 0,
            mcp_servers: vec![],
            load: ServitorLoad::default(),
            stats: ServitorStats::default(),
            last_task_ts: None,
        }
    }
}

/// MCP server health snapshot for profile publishing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    #[serde(default)]
    pub status: McpServerHealth,
}

/// MCP server health state.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpServerHealth {
    Healthy,
    Degraded,
    #[default]
    Unavailable,
}

/// Current servitor work load.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServitorLoad {
    pub tasks_executing: u64,
    pub tasks_queued: u64,
}

impl ServitorLoad {
    pub fn is_empty(&self) -> bool {
        self.tasks_executing == 0 && self.tasks_queued == 0
    }
}

/// Cumulative runtime counters for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServitorStats {
    pub tasks_offered: u64,
    pub tasks_executed: u64,
    pub tasks_failed: u64,
}

impl ServitorStats {
    pub fn is_empty(&self) -> bool {
        self.tasks_offered == 0 && self.tasks_executed == 0 && self.tasks_failed == 0
    }
}

/// Scope constraints for a capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConstraints {
    pub allow: Vec<String>,
    pub block: Vec<String>,
}

/// Task-scoped credential override.
///
/// This can only further restrict what a task is allowed to do.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskScopeOverride {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub block: Vec<String>,
}

impl TaskScopeOverride {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.block.is_empty()
    }
}

/// Resource limits for this Servitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: u32,
    pub memory_mb: u64,
}

impl ResourceLimits {
    /// Detect actual system resources from /proc.
    /// Falls back to conservative defaults if detection fails.
    pub fn detect() -> Self {
        let cpu = Self::detect_cpu_cores().unwrap_or(2);
        let memory_mb = Self::detect_memory_mb().unwrap_or(4096);
        Self { cpu, memory_mb }
    }

    /// Count CPU cores from /proc/cpuinfo.
    fn detect_cpu_cores() -> Option<u32> {
        let content = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        let count = content
            .lines()
            .filter(|line| line.starts_with("processor"))
            .count();
        if count > 0 {
            Some(count as u32)
        } else {
            None
        }
    }

    /// Get total memory in MB from /proc/meminfo.
    fn detect_memory_mb() -> Option<u64> {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                // Format: "MemTotal:       16384000 kB"
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        return Some(kb / 1024); // Convert kB to MB
                    }
                }
            }
        }
        None
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::detect()
    }
}

/// Task claim message — claim a task before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskClaim {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Correlation ID for tracking.
    pub correlation_id: String,

    /// Hash of the task being claimed.
    pub task_hash: String,

    /// Servitor claiming the task.
    pub servitor_id: PublicId,

    /// Time-to-live in seconds.
    pub ttl_seconds: u64,

    /// Claim timestamp.
    pub timestamp: DateTime<Utc>,
}

impl TaskClaim {
    pub fn new(task_hash: String, servitor_id: PublicId, ttl_seconds: u64) -> Self {
        Self {
            msg_type: "task_claim".to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            task_hash,
            servitor_id,
            ttl_seconds,
            timestamp: Utc::now(),
        }
    }
}

/// Task result with signed attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Stable task identifier for offer/assign lifecycle.
    pub task_id: String,

    /// Servitor that executed the task.
    pub servitor: PublicId,

    /// Correlation ID matching the claim.
    pub correlation_id: String,

    /// Hash of the task.
    pub task_hash: String,

    /// Hash of the result content.
    pub result_hash: String,

    /// Execution status.
    pub status: TaskStatus,

    /// Result content (when successful).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error message (when failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Execution duration in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,

    /// Signed attestation.
    pub attestation: Attestation,

    /// Distributed trace identifier for this task execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// Task execution status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Success,
    Error,
    Timeout,
}

/// Signed attestation binding identity to output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Servitor that executed the task.
    pub servitor_id: PublicId,

    /// Ed25519 signature of the result hash.
    pub signature: String,

    /// Attestation timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Task message received from egregore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Stable task identifier, distinct from message hash when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Task hash (computed by sender).
    pub hash: String,

    /// Task class used for authorization and assignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,

    /// Original request text, if separately supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,

    /// Requestor identity for assignment authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requestor: Option<PublicId>,

    /// Task prompt/instruction.
    pub prompt: String,

    /// Required capabilities to execute this task.
    #[serde(default)]
    pub required_caps: Vec<String>,

    /// Parent message hash (for threading/context).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Additional context.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,

    /// Optional per-task scope restriction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_override: Option<TaskScopeOverride>,

    /// Task priority (higher = more urgent).
    #[serde(default)]
    pub priority: i32,

    /// Task timeout in seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Author identity (egregore pubkey). Set during intake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Resolved keeper name (set after authorization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keeper: Option<String>,
}

impl Task {
    /// Normalize optional protocol fields so downstream code can rely on them.
    pub fn normalize(&mut self, author: Option<&PublicId>) {
        if self.id.is_none() {
            self.id = Some(self.hash.clone());
        }
        if self.request.is_none() {
            self.request = Some(self.prompt.clone());
        }
        if self.task_type.is_none() {
            self.task_type = self.required_caps.first().cloned();
        }
        if self.requestor.is_none() {
            self.requestor = author.cloned();
        }
    }

    pub fn effective_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.hash)
    }

    pub fn effective_task_type(&self) -> &str {
        self.task_type
            .as_deref()
            .or_else(|| self.required_caps.first().map(String::as_str))
            .unwrap_or("general")
    }

    pub fn effective_request(&self) -> &str {
        self.request.as_deref().unwrap_or(&self.prompt)
    }
}

/// Servitor offering to execute a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOffer {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    pub capabilities: Vec<String>,
    pub ttl_seconds: u64,
    pub timestamp: DateTime<Utc>,
}

impl TaskOffer {
    pub fn new(
        task_id: impl Into<String>,
        servitor: PublicId,
        capabilities: Vec<String>,
        ttl_seconds: u64,
    ) -> Self {
        Self {
            msg_type: "task_offer".to_string(),
            task_id: task_id.into(),
            servitor,
            capabilities,
            ttl_seconds,
            timestamp: Utc::now(),
        }
    }
}

/// Assignment selecting a specific servitor for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssign {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigner: Option<PublicId>,
}

/// Acknowledgment that execution has started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStarted {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    pub eta_seconds: u64,
    pub timestamp: DateTime<Utc>,
}

impl TaskStarted {
    pub fn new(task_id: impl Into<String>, servitor: PublicId, eta_seconds: u64) -> Self {
        Self {
            msg_type: "task_started".to_string(),
            task_id: task_id.into(),
            servitor,
            eta_seconds,
            timestamp: Utc::now(),
        }
    }
}

/// Offer withdrawal before execution starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOfferWithdraw {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl TaskOfferWithdraw {
    pub fn new(task_id: impl Into<String>, servitor: PublicId, reason: Option<String>) -> Self {
        Self {
            msg_type: "task_offer_withdraw".to_string(),
            task_id: task_id.into(),
            servitor,
            reason,
            timestamp: Utc::now(),
        }
    }
}

/// Request a status update for an executing task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPing {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requestor: Option<PublicId>,
}

/// Execution progress update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_eta_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl TaskStatusMessage {
    pub fn new(
        task_id: impl Into<String>,
        servitor: PublicId,
        revised_eta_seconds: Option<u64>,
        message: Option<String>,
    ) -> Self {
        Self {
            msg_type: "task_status".to_string(),
            task_id: task_id.into(),
            servitor,
            progress_pct: None,
            revised_eta_seconds,
            message,
            timestamp: Utc::now(),
        }
    }
}

/// Failure reasons for task lifecycle errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskFailureReason {
    NoResponse,
    ExecutionError,
    Timeout,
}

/// Task failure message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFailed {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub servitor: PublicId,
    pub reason: TaskFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl TaskFailed {
    pub fn new(
        task_id: impl Into<String>,
        servitor: PublicId,
        reason: TaskFailureReason,
        details: Option<String>,
    ) -> Self {
        Self {
            msg_type: "task_failed".to_string(),
            task_id: task_id.into(),
            servitor,
            reason,
            details,
            timestamp: Utc::now(),
        }
    }
}

/// Notification message for outbound communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Servitor sending the notification.
    pub servitor_id: PublicId,

    /// Target channel (e.g., "discord:channel:inbox-alerts").
    pub channel: String,

    /// Priority level.
    pub priority: NotificationPriority,

    /// Notification title.
    pub title: String,

    /// Notification body.
    pub body: String,

    /// Source of the notification.
    pub source: String,

    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for NotificationPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Published trace span for task execution and MCP tool activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Distributed trace identifier shared across related spans.
    pub trace_id: String,

    /// Unique identifier for this span.
    pub span_id: String,

    /// Optional parent span identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,

    /// Human-readable span name.
    pub name: String,

    /// Service or node responsible for the span.
    pub service: String,

    /// Span start timestamp.
    pub start_ts: DateTime<Utc>,

    /// Span end timestamp.
    pub end_ts: DateTime<Utc>,

    /// Execution outcome.
    pub status: TraceSpanStatus,

    /// Additional span attributes.
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,

    /// Time-stamped events recorded during the span.
    #[serde(default)]
    pub events: Vec<TraceEvent>,
}

impl TraceSpan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        trace_id: impl Into<String>,
        span_id: impl Into<String>,
        parent_span_id: Option<String>,
        name: impl Into<String>,
        service: impl Into<String>,
        start_ts: DateTime<Utc>,
        end_ts: DateTime<Utc>,
        status: TraceSpanStatus,
    ) -> Self {
        Self {
            msg_type: "trace_span".to_string(),
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_span_id,
            name: name.into(),
            service: service.into(),
            start_ts,
            end_ts,
            status,
            attributes: HashMap::new(),
            events: Vec::new(),
        }
    }
}

/// Authorization gate where a denial occurred.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthGate {
    Offer,
    Assignment,
}

/// Authorization denial event published for audit/debug visibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDenied {
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Servitor that denied the request.
    pub servitor_id: PublicId,

    /// Denied person identity or redacted identity hint.
    pub person_id: String,

    /// Origin place that was checked.
    pub place: String,

    /// Requested skill or wildcard scope.
    pub skill: String,

    /// Authorization gate where the denial occurred.
    pub gate: AuthGate,

    /// Human-readable denial reason.
    pub reason: String,

    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

impl AuthDenied {
    pub fn new(
        servitor_id: PublicId,
        person_id: String,
        place: String,
        skill: String,
        gate: AuthGate,
        reason: String,
    ) -> Self {
        Self {
            msg_type: "auth_denied".to_string(),
            servitor_id,
            person_id,
            place,
            skill,
            gate,
            reason,
            timestamp: Utc::now(),
        }
    }
}

/// Execution outcome for a trace span.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TraceSpanStatus {
    Ok,
    Error,
    Timeout,
}

/// Event emitted within a trace span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub ts: DateTime<Utc>,
    pub name: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, serde_json::Value>,
}
/// Generic egregore message envelope (for hook input and context fetching).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgregoreMessage {
    pub author: PublicId,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub content: serde_json::Value,
    pub hash: String,
    pub signature: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl EgregoreMessage {
    /// Extract the content type from the message.
    pub fn content_type(&self) -> Option<&str> {
        self.content.get("type").and_then(|v| v.as_str())
    }

    /// Try to parse content as a Task.
    pub fn as_task(&self) -> Option<Task> {
        if self.content_type() == Some("task") {
            serde_json::from_value(self.content.clone()).ok()
        } else {
            None
        }
    }

    /// Try to parse content as a TaskResult.
    pub fn as_task_result(&self) -> Option<TaskResult> {
        if self.content_type() == Some("task_result") {
            serde_json::from_value(self.content.clone()).ok()
        } else {
            None
        }
    }

    pub fn as_task_assign(&self) -> Option<TaskAssign> {
        if self.content_type() == Some("task_assign") {
            serde_json::from_value(self.content.clone()).ok()
        } else {
            None
        }
    }

    pub fn as_task_ping(&self) -> Option<TaskPing> {
        if self.content_type() == Some("task_ping") {
            serde_json::from_value(self.content.clone()).ok()
        } else {
            None
        }
    }

    /// Get the prompt if this is a task message.
    pub fn prompt(&self) -> Option<&str> {
        self.content.get("prompt").and_then(|v| v.as_str())
    }

    /// Get the parent_id if present.
    pub fn parent_id(&self) -> Option<&str> {
        self.content.get("parent_id").and_then(|v| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn resource_detection_returns_nonzero() {
        let limits = ResourceLimits::detect();
        // Should detect at least 1 CPU and some memory
        assert!(limits.cpu >= 1, "CPU count should be at least 1");
        assert!(limits.memory_mb >= 512, "Memory should be at least 512 MB");
        println!(
            "Detected: {} CPUs, {} MB memory",
            limits.cpu, limits.memory_mb
        );
    }

    #[test]
    fn resource_limits_default_uses_detection() {
        let defaults = ResourceLimits::default();
        let detected = ResourceLimits::detect();
        assert_eq!(defaults.cpu, detected.cpu);
        assert_eq!(defaults.memory_mb, detected.memory_mb);
    }

    #[test]
    fn task_scope_override_roundtrips() {
        let task: Task = serde_json::from_value(serde_json::json!({
            "type": "task",
            "hash": "task-123",
            "prompt": "inspect logs",
            "scope_override": {
                "allow": ["shell:read:*"],
                "block": ["shell:execute:*"]
            }
        }))
        .unwrap();

        let scope_override = task.scope_override.unwrap();
        assert_eq!(scope_override.allow, vec!["shell:read:*"]);
        assert_eq!(scope_override.block, vec!["shell:execute:*"]);
    }

    #[test]
    fn task_scope_override_rejects_unknown_fields() {
        let error = serde_json::from_value::<Task>(serde_json::json!({
            "type": "task",
            "hash": "task-123",
            "prompt": "inspect logs",
            "scope_override": {
                "allow": ["shell:read:*"],
                "unknown": true
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn profile_defaults_include_operational_fields() {
        let profile = ServitorProfile::new(PublicId("@test.ed25519".to_string()), 10000);

        assert_eq!(profile.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(profile.uptime_secs, 0);
        assert!(profile.mcp_servers.is_empty());
        assert_eq!(profile.load, ServitorLoad::default());
        assert_eq!(profile.stats, ServitorStats::default());
        assert!(profile.last_task_ts.is_none());
    }

    #[test]
    fn profile_serializes_enhanced_monitoring_fields() {
        let mut profile = ServitorProfile::new(PublicId("@test.ed25519".to_string()), 10000);
        profile.uptime_secs = 42;
        profile.mcp_servers = vec![McpServerStatus {
            name: "shell".to_string(),
            transport: "stdio".to_string(),
            status: McpServerHealth::Healthy,
        }];
        profile.load = ServitorLoad {
            tasks_executing: 1,
            tasks_queued: 2,
        };
        profile.stats = ServitorStats {
            tasks_offered: 3,
            tasks_executed: 2,
            tasks_failed: 1,
        };
        profile.last_task_ts = Some(Utc::now());

        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["uptime_secs"], 42);
        assert_eq!(json["mcp_servers"][0]["name"], "shell");
        assert_eq!(json["mcp_servers"][0]["status"], "healthy");
        assert_eq!(json["load"]["tasks_executing"], 1);
        assert_eq!(json["load"]["tasks_queued"], 2);
        assert_eq!(json["stats"]["tasks_offered"], 3);
        assert_eq!(json["stats"]["tasks_executed"], 2);
        assert_eq!(json["stats"]["tasks_failed"], 1);
        assert!(json["last_task_ts"].is_string());
    }

    #[test]
    fn auth_denied_roundtrip() {
        let identity = Identity::generate();
        let denial = AuthDenied::new(
            identity.public_id(),
            "discord:123".to_string(),
            "discord:guild:channel".to_string(),
            "shell:execute".to_string(),
            AuthGate::Offer,
            "no matching permission".to_string(),
        );

        let json = serde_json::to_string(&denial).unwrap();
        let parsed: AuthDenied = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.msg_type, "auth_denied");
        assert_eq!(parsed.gate, AuthGate::Offer);
        assert_eq!(parsed.person_id, "discord:123");
    }
}
