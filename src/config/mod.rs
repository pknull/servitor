//! Configuration loading and validation.

mod schema;

pub use schema::*;

use crate::error::{Result, ServitorError};
use std::path::Path;

impl Config {
    /// Load configuration from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| ServitorError::Config {
            reason: format!("failed to read config file {}: {}", path.display(), e),
        })?;
        Self::from_str(&content)
    }

    /// Parse configuration from a TOML string.
    pub fn from_str(content: &str) -> Result<Self> {
        let config: Config = toml::from_str(content).map_err(|e| ServitorError::Config {
            reason: format!("failed to parse config: {}", e),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration.
    fn validate(&self) -> Result<()> {
        // Validate LLM provider
        match self.llm.provider.as_str() {
            "anthropic" => {
                if self.llm.api_key_env.is_none() {
                    return Err(ServitorError::Config {
                        reason: "anthropic provider requires api_key_env".into(),
                    });
                }
            }
            "openai" => {
                if self.llm.api_key_env.is_none() {
                    return Err(ServitorError::Config {
                        reason: "openai provider requires api_key_env".into(),
                    });
                }
            }
            "ollama" => {
                // Ollama doesn't require API key
            }
            "openai-compat" => {
                if self.llm.base_url.is_none() {
                    return Err(ServitorError::Config {
                        reason: "openai-compat provider requires base_url".into(),
                    });
                }
            }
            "codex" => {
                if self.llm.token_file.is_none() {
                    return Err(ServitorError::Config {
                        reason: "codex provider requires token_file".into(),
                    });
                }
            }
            "claude-code" => {
                // Claude Code uses CLI authentication, no config needed
            }
            other => {
                return Err(ServitorError::Config {
                    reason: format!("unknown LLM provider: {}", other),
                });
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.llm.provider, "anthropic");
        assert_eq!(config.llm.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
[identity]
data_dir = "~/.servitor"

[egregore]
api_url = "http://127.0.0.1:7654"
subscribe = true

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

[mcp.shell]
transport = "stdio"
command = "mcp-server-shell"
scope.allow = ["execute:~/scripts/*"]
scope.block = ["execute:/etc/*"]
on_notification = "Handle event: {{notification}}"

[agent]
max_turns = 50
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

[[schedule]]
name = "test-task"
cron = "0 * * * * *"
task = "Test task"
publish = true
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.mcp.len(), 1);
        assert!(config.mcp.contains_key("shell"));
        assert!(config.egregore.subscribe);
        assert_eq!(config.schedule.len(), 1);
        assert_eq!(config.task.offer_ttl_secs, 300);
    }

    #[test]
    fn reject_missing_api_key() {
        let toml = r#"
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_stdio_without_command() {
        let toml = r#"
[llm]
provider = "ollama"
model = "llama3.3:70b"

[mcp.shell]
transport = "stdio"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_cron() {
        let toml = r#"
[llm]
provider = "ollama"
model = "llama3.3:70b"

[[schedule]]
name = "bad-cron"
cron = "not a cron expression"
task = "Test"
"#;
        let result = Config::from_str(toml);
        assert!(result.is_err());
    }
}
