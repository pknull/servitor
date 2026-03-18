//! Prometheus metrics for Servitor observability.
//!
//! ## Cardinality Rules
//!
//! Per component-model.md, metrics labels must be bounded:
//! - `tool_name` - bounded (OK)
//! - `mcp_server` - bounded (OK)
//! - `task_type` - bounded (OK)
//! - `status` - bounded (OK)
//! - `decision` - bounded (OK)
//! - `provider` - bounded (OK)
//! - `task_id` - UNBOUNDED (DO NOT use as label)
//! - `keeper` - potentially high cardinality (omitted)

use std::net::SocketAddr;
use std::time::Instant;

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;

use crate::config::MetricsConfig;
use crate::error::{Result, ServitorError};

/// Initialize the Prometheus metrics exporter.
///
/// Spawns an HTTP server at the configured bind address exposing `/metrics`.
pub fn init(config: &MetricsConfig) -> Result<()> {
    if !config.enabled {
        tracing::debug!("metrics disabled");
        return Ok(());
    }

    let addr: SocketAddr = config.bind.parse().map_err(|e| ServitorError::Config {
        reason: format!("invalid metrics bind address '{}': {}", config.bind, e),
    })?;

    PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()
        .map_err(|e| ServitorError::Config {
            reason: format!("failed to install metrics exporter: {}", e),
        })?;

    tracing::info!(bind = %config.bind, "metrics endpoint enabled");
    Ok(())
}

// ============================================================================
// Task metrics
// ============================================================================

/// Record a task completion.
pub fn record_task_complete(task_type: &str, status: TaskStatus) {
    counter!(
        "servitor_tasks_total",
        "status" => status.as_str(),
        "task_type" => task_type.to_string()
    )
    .increment(1);
}

/// Record task duration.
pub fn record_task_duration(task_type: &str, duration_secs: f64) {
    histogram!(
        "servitor_task_duration_seconds",
        "task_type" => task_type.to_string()
    )
    .record(duration_secs);
}

/// Set the current number of active tasks.
pub fn set_active_tasks(count: u64) {
    gauge!("servitor_active_tasks").set(count as f64);
}

// ============================================================================
// Tool call metrics
// ============================================================================

/// Record a tool call completion.
pub fn record_tool_call(tool_name: &str, mcp_server: &str, status: ToolCallStatus) {
    counter!(
        "servitor_tool_calls_total",
        "tool_name" => tool_name.to_string(),
        "mcp_server" => mcp_server.to_string(),
        "status" => status.as_str()
    )
    .increment(1);
}

/// Record tool call duration.
pub fn record_tool_call_duration(tool_name: &str, duration_secs: f64) {
    histogram!(
        "servitor_tool_call_duration_seconds",
        "tool_name" => tool_name.to_string()
    )
    .record(duration_secs);
}

// ============================================================================
// Authorization metrics
// ============================================================================

/// Record an authorization decision.
pub fn record_auth_decision(decision: AuthDecision) {
    counter!(
        "servitor_auth_decisions_total",
        "decision" => decision.as_str()
    )
    .increment(1);
}

// ============================================================================
// LLM metrics
// ============================================================================

/// Record LLM call latency.
pub fn record_llm_latency(provider: &str, duration_secs: f64) {
    histogram!(
        "servitor_llm_latency_seconds",
        "provider" => provider.to_string()
    )
    .record(duration_secs);
}

// ============================================================================
// MCP server metrics
// ============================================================================

/// Set the number of connected MCP servers.
pub fn set_mcp_servers_connected(count: u64) {
    gauge!("servitor_mcp_servers_connected").set(count as f64);
}

// ============================================================================
// Status enums
// ============================================================================

/// Task completion status for metrics.
#[derive(Debug, Clone, Copy)]
pub enum TaskStatus {
    Success,
    Error,
    Timeout,
}

impl TaskStatus {
    fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Success => "success",
            TaskStatus::Error => "error",
            TaskStatus::Timeout => "timeout",
        }
    }
}

impl From<&crate::egregore::TaskStatus> for TaskStatus {
    fn from(status: &crate::egregore::TaskStatus) -> Self {
        match status {
            crate::egregore::TaskStatus::Success => TaskStatus::Success,
            crate::egregore::TaskStatus::Error => TaskStatus::Error,
            crate::egregore::TaskStatus::Timeout => TaskStatus::Timeout,
        }
    }
}

/// Tool call status for metrics.
#[derive(Debug, Clone, Copy)]
pub enum ToolCallStatus {
    Success,
    Error,
    ScopeViolation,
    Unauthorized,
}

impl ToolCallStatus {
    fn as_str(&self) -> &'static str {
        match self {
            ToolCallStatus::Success => "success",
            ToolCallStatus::Error => "error",
            ToolCallStatus::ScopeViolation => "scope_violation",
            ToolCallStatus::Unauthorized => "unauthorized",
        }
    }
}

/// Authorization decision for metrics.
#[derive(Debug, Clone, Copy)]
pub enum AuthDecision {
    Allowed,
    Denied,
    OpenMode,
}

impl AuthDecision {
    fn as_str(&self) -> &'static str {
        match self {
            AuthDecision::Allowed => "allowed",
            AuthDecision::Denied => "denied",
            AuthDecision::OpenMode => "open_mode",
        }
    }
}

// ============================================================================
// Timer helper
// ============================================================================

/// A timer for measuring operation duration.
pub struct Timer {
    start: Instant,
}

impl Timer {
    /// Start a new timer.
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed time in seconds.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_string_values() {
        assert_eq!(TaskStatus::Success.as_str(), "success");
        assert_eq!(TaskStatus::Error.as_str(), "error");
        assert_eq!(TaskStatus::Timeout.as_str(), "timeout");
    }

    #[test]
    fn tool_call_status_string_values() {
        assert_eq!(ToolCallStatus::Success.as_str(), "success");
        assert_eq!(ToolCallStatus::Error.as_str(), "error");
        assert_eq!(ToolCallStatus::ScopeViolation.as_str(), "scope_violation");
        assert_eq!(ToolCallStatus::Unauthorized.as_str(), "unauthorized");
    }

    #[test]
    fn auth_decision_string_values() {
        assert_eq!(AuthDecision::Allowed.as_str(), "allowed");
        assert_eq!(AuthDecision::Denied.as_str(), "denied");
        assert_eq!(AuthDecision::OpenMode.as_str(), "open_mode");
    }

    #[test]
    fn timer_measures_elapsed_time() {
        let timer = Timer::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.elapsed_secs();
        assert!(elapsed >= 0.01);
        assert!(elapsed < 1.0);
    }

    #[test]
    fn disabled_config_does_not_start_server() {
        let config = MetricsConfig {
            enabled: false,
            bind: "127.0.0.1:0".to_string(),
        };
        // Should return Ok without starting server
        assert!(init(&config).is_ok());
    }
}
