//! Configuration loading and validation.

mod schema;

pub use schema::*;

use crate::error::{Result, ServitorError};
use std::path::Path;

impl Config {
    /// Create a minimal default configuration for testing or fallback.
    ///
    /// This configuration uses sensible defaults for local development:
    /// - Local egregore at 127.0.0.1:7654
    /// - Standard executor timeout and task lifecycle settings
    pub fn minimal_defaults() -> Result<Self> {
        let toml = r#"
[identity]
data_dir = "~/.servitor"

[egregore]
api_url = "http://127.0.0.1:7654"
subscribe = false

[agent]
timeout_secs = 300

[task]
offer_ttl_secs = 300
offer_timeout_secs = 60
assign_timeout_secs = 300
start_timeout_secs = 30
eta_buffer_multiplier = 1.5
ping_timeout_secs = 30

[heartbeat]
interval_secs = 300
include_runtime_monitoring = false

[profile]
roles = ["executor"]
"#;
        Self::from_str(toml)
    }

    /// Load configuration from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| ServitorError::Config {
            reason: format!("failed to read config file {}: {}", path.display(), e),
        })?;
        Self::from_str(&content)
    }

    /// Parse configuration from a TOML string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(content: &str) -> Result<Self> {
        let config: Config = toml::from_str(content).map_err(|e| ServitorError::Config {
            reason: format!("failed to parse config: {}", e),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration.
    fn validate(&self) -> Result<()> {
        // Validate MCP server configs
        for (name, mcp) in &self.mcp {
            match mcp.transport.as_str() {
                "stdio" => {
                    if mcp.command.is_none() {
                        return Err(ServitorError::Config {
                            reason: format!(
                                "MCP server '{}' with stdio transport requires command",
                                name
                            ),
                        });
                    }
                }
                "http" => {
                    if mcp.url.is_none() {
                        return Err(ServitorError::Config {
                            reason: format!(
                                "MCP server '{}' with http transport requires url",
                                name
                            ),
                        });
                    }
                }
                other => {
                    return Err(ServitorError::Config {
                        reason: format!("MCP server '{}' has unknown transport: {}", name, other),
                    });
                }
            }

            validate_optional_tool_calls(
                &format!("mcp.{}.on_notification", name),
                &mcp.on_notification,
            )?;

            if !mcp.on_notification.is_empty() && mcp.transport != "stdio" {
                return Err(ServitorError::Config {
                    reason: format!(
                        "mcp.{}.on_notification currently requires stdio transport",
                        name
                    ),
                });
            }
        }

        // Validate scheduled tasks
        for task in &self.schedule {
            if task.cron.parse::<cron::Schedule>().is_err() {
                return Err(ServitorError::Config {
                    reason: format!(
                        "scheduled task '{}' has invalid cron expression: {}",
                        task.name, task.cron
                    ),
                });
            }

            validate_tool_calls(&format!("schedule '{}'", task.name), &task.tool_calls)?;
        }

        // Validate watcher tasks
        for watch in &self.watch {
            validate_tool_calls(&format!("watch '{}'", watch.name), &watch.tool_calls)?;
            let mcp = self
                .mcp
                .get(&watch.mcp)
                .ok_or_else(|| ServitorError::Config {
                    reason: format!(
                        "watch '{}' references unknown MCP server '{}'",
                        watch.name, watch.mcp
                    ),
                })?;
            if mcp.transport != "stdio" {
                return Err(ServitorError::Config {
                    reason: format!(
                        "watch '{}' requires stdio MCP transport for server '{}'",
                        watch.name, watch.mcp
                    ),
                });
            }
        }

        let mut target_ids = std::collections::HashSet::new();
        for target in &self.profile.targets {
            if target.target_id.trim().is_empty() {
                return Err(ServitorError::Config {
                    reason: "profile.targets[].target_id requires a non-empty value".into(),
                });
            }
            if target.kind.trim().is_empty() {
                return Err(ServitorError::Config {
                    reason: format!(
                        "profile target '{}' requires a non-empty kind",
                        target.target_id
                    ),
                });
            }
            if !target_ids.insert(target.target_id.clone()) {
                return Err(ServitorError::Config {
                    reason: format!(
                        "profile.targets contains duplicate target_id '{}'",
                        target.target_id
                    ),
                });
            }

            validate_optional_tool_calls(
                &format!("profile target '{}'.snapshot_tool_calls", target.target_id),
                &target.snapshot_tool_calls,
            )?;
        }

        Ok(())
    }

    /// Expand shell paths in configuration (e.g., ~ to home directory).
    pub fn expand_paths(&mut self) {
        self.identity.data_dir = expand_path(&self.identity.data_dir);

        for mcp in self.mcp.values_mut() {
            for pattern in &mut mcp.scope.allow {
                *pattern = expand_path(pattern);
            }
            for pattern in &mut mcp.scope.block {
                *pattern = expand_path(pattern);
            }
        }
    }
}

/// Expand shell-style paths (~ and environment variables).
fn expand_path(path: &str) -> String {
    shellexpand::full(path)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn validate_tool_calls(label: &str, tool_calls: &[ToolCallTemplate]) -> Result<()> {
    if tool_calls.is_empty() {
        return Err(ServitorError::Config {
            reason: format!("{label} requires at least one tool_call"),
        });
    }

    for (index, call) in tool_calls.iter().enumerate() {
        if call.name.trim().is_empty() {
            return Err(ServitorError::Config {
                reason: format!("{label} tool_call[{index}] requires a non-empty name"),
            });
        }
    }

    Ok(())
}

fn validate_optional_tool_calls(label: &str, tool_calls: &[ToolCallTemplate]) -> Result<()> {
    if tool_calls.is_empty() {
        return Ok(());
    }

    validate_tool_calls(label, tool_calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
[agent]
timeout_secs = 120
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.agent.timeout_secs, 120);
        assert_eq!(config.heartbeat.interval_secs, 300);
    }

    #[test]
    fn parse_executor_config() {
        let toml = r#"
[a2a_server]
enabled = true
bind = "0.0.0.0:8765"
name = "shell-worker"

[mcp.shell]
transport = "stdio"
command = "mcp-server-shell"
"#;
        let config = Config::from_str(toml).unwrap();
        assert!(config
            .a2a_server
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false));
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
[identity]
data_dir = "~/.servitor"

[egregore]
api_url = "http://127.0.0.1:7654"
subscribe = true

[mcp.shell]
transport = "stdio"
command = "mcp-server-shell"
scope.allow = ["execute:~/scripts/*"]
scope.block = ["execute:/etc/*"]
on_notification = [
  { name = "shell__execute", arguments = { command = "echo {{notification}}" } }
]

[agent]
timeout_secs = 300

[task]
offer_ttl_secs = 300
offer_timeout_secs = 60
assign_timeout_secs = 300
start_timeout_secs = 30
eta_buffer_multiplier = 1.5
ping_timeout_secs = 30

[heartbeat]
interval_secs = 10

[profile]
roles = ["docker-host", "staging"]
labels = { env = "staging", site = "lab-a" }

[[profile.targets]]
target_id = "staging-web"
kind = "docker_compose_project"
summary = "Primary staging stack"
roles = ["staging", "web"]
snapshot_ttl_secs = 120
snapshot_tool_calls = [
  { name = "shell__execute", arguments = { command = "docker compose -p staging-web ps --format json" } }
]

[[schedule]]
name = "test-task"
cron = "0 * * * * *"
prompt = "Test task"
tool_calls = [
  { name = "shell__execute", arguments = { command = "echo test" } }
]
publish = true
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.mcp.len(), 1);
        assert!(config.mcp.contains_key("shell"));
        assert!(config.egregore.subscribe);
        assert_eq!(config.schedule.len(), 1);
        assert_eq!(config.task.offer_ttl_secs, 300);
        assert_eq!(config.profile.roles, vec!["docker-host", "staging"]);
        assert_eq!(
            config.profile.labels.get("env").map(String::as_str),
            Some("staging")
        );
        assert_eq!(config.profile.targets.len(), 1);
        assert_eq!(config.profile.targets[0].target_id, "staging-web");
        assert_eq!(config.profile.targets[0].snapshot_ttl_secs, 120);
        assert_eq!(config.profile.targets[0].snapshot_tool_calls.len(), 1);
    }

    #[test]
    fn reject_stdio_without_command() {
        let toml = r#"
[mcp.shell]
transport = "stdio"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_cron() {
        let toml = r#"
[[schedule]]
name = "bad-cron"
cron = "not a cron expression"
tool_calls = [
  { name = "shell__execute", arguments = { command = "echo test" } }
]
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_schedule_without_tool_calls() {
        let toml = r#"
[[schedule]]
name = "missing-tool-calls"
cron = "0 * * * * *"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_watch_unknown_server() {
        let toml = r#"
[[watch]]
name = "bad-watch"
mcp = "missing"
event = "file_changed"
tool_calls = [
  { name = "shell__execute", arguments = { command = "echo test" } }
]
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_duplicate_profile_targets() {
        let toml = r#"
[profile]
[[profile.targets]]
target_id = "staging-web"
kind = "docker_compose_project"

[[profile.targets]]
target_id = "staging-web"
kind = "docker_compose_project"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("duplicate target_id 'staging-web'"));
    }
}
