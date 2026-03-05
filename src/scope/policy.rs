//! Scope enforcement policy — allow/block logic.

use crate::config::ScopeConfig;
use crate::error::{Result, ServitorError};
use crate::scope::matcher::{parse_scoped_pattern, ScopeMatcher};

/// Compiled scope policy for an MCP server.
#[derive(Debug)]
pub struct ScopePolicy {
    #[allow(dead_code)]
    server_name: String,
    allow: Vec<(String, ScopeMatcher)>,
    block: Vec<(String, ScopeMatcher)>,
}

impl ScopePolicy {
    /// Compile a scope policy from configuration.
    pub fn from_config(server_name: &str, config: &ScopeConfig) -> Result<Self> {
        let mut allow = Vec::new();
        let mut block = Vec::new();

        for pattern in &config.allow {
            let (scope, pat) = parse_scoped_pattern(pattern);
            allow.push((scope.to_string(), ScopeMatcher::new(pat)?));
        }

        for pattern in &config.block {
            let (scope, pat) = parse_scoped_pattern(pattern);
            block.push((scope.to_string(), ScopeMatcher::new(pat)?));
        }

        Ok(Self {
            server_name: server_name.to_string(),
            allow,
            block,
        })
    }

    /// Check if a tool call is allowed.
    ///
    /// Logic:
    /// 1. If blocked by any block pattern, deny (block takes precedence)
    /// 2. If allowed by any allow pattern, permit
    /// 3. If no allow patterns defined, permit by default
    /// 4. If allow patterns exist but none match, deny
    pub fn check(&self, tool_name: &str, args: &serde_json::Value) -> Result<()> {
        // Extract key arguments for matching
        let targets = extract_targets(tool_name, args);

        // Check block patterns first (they take precedence)
        for (scope, matcher) in &self.block {
            for target in &targets {
                if scope_matches(scope, tool_name) && matcher.matches(target) {
                    return Err(ServitorError::ScopeViolation {
                        reason: format!(
                            "blocked by policy: {} matches block pattern '{}:{}'",
                            target,
                            scope,
                            matcher.pattern()
                        ),
                    });
                }
            }
        }

        // If no allow patterns, permit by default
        if self.allow.is_empty() {
            return Ok(());
        }

        // Check allow patterns
        for (scope, matcher) in &self.allow {
            // Wildcard scope or matching tool name
            if scope_matches(scope, tool_name) {
                // Wildcard pattern or matching target
                if matcher.pattern() == "*" {
                    return Ok(());
                }
                for target in &targets {
                    if matcher.matches(target) {
                        return Ok(());
                    }
                }
            }
        }

        // No allow pattern matched
        Err(ServitorError::ScopeViolation {
            reason: format!(
                "not allowed: tool '{}' with targets {:?} not permitted by any allow pattern",
                tool_name, targets
            ),
        })
    }
}

/// Check if a scope matches a tool name.
fn scope_matches(scope: &str, tool_name: &str) -> bool {
    scope == "*" || scope == tool_name || tool_name.starts_with(scope)
}

/// Extract target strings from tool arguments for pattern matching.
fn extract_targets(tool_name: &str, args: &serde_json::Value) -> Vec<String> {
    let mut targets = Vec::new();

    // Common patterns for extracting paths/targets from arguments
    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        targets.push(path.to_string());
    }
    if let Some(file) = args.get("file").and_then(|v| v.as_str()) {
        targets.push(file.to_string());
    }
    if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
        targets.push(command.to_string());
    }
    if let Some(container) = args.get("container").and_then(|v| v.as_str()) {
        targets.push(container.to_string());
    }
    if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
        targets.push(name.to_string());
    }

    // If no specific targets found, use the tool name as a fallback
    if targets.is_empty() {
        targets.push(tool_name.to_string());
    }

    targets
}

/// Scope enforcer that manages policies for multiple MCP servers.
#[derive(Debug, Default)]
pub struct ScopeEnforcer {
    policies: std::collections::HashMap<String, ScopePolicy>,
}

impl ScopeEnforcer {
    /// Create a new scope enforcer.
    pub fn new() -> Self {
        Self {
            policies: std::collections::HashMap::new(),
        }
    }

    /// Add a policy for an MCP server.
    pub fn add_policy(&mut self, server_name: &str, config: &ScopeConfig) -> Result<()> {
        let policy = ScopePolicy::from_config(server_name, config)?;
        self.policies.insert(server_name.to_string(), policy);
        Ok(())
    }

    /// Check if a tool call is allowed.
    pub fn check(
        &self,
        mcp_name: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        if let Some(policy) = self.policies.get(mcp_name) {
            policy.check(tool_name, args)
        } else {
            // No policy defined, allow by default
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ScopeConfig {
        ScopeConfig {
            allow: vec![
                "execute:~/scripts/*".to_string(),
                "*:*.txt".to_string(),
            ],
            block: vec![
                "execute:/etc/*".to_string(),
                "execute:rm *".to_string(),
            ],
        }
    }

    #[test]
    fn allow_matching_pattern() {
        let policy = ScopePolicy::from_config("shell", &test_config()).unwrap();

        let args = serde_json::json!({ "command": "~/scripts/backup.sh" });
        assert!(policy.check("execute", &args).is_ok());
    }

    #[test]
    fn block_takes_precedence() {
        let policy = ScopePolicy::from_config("shell", &test_config()).unwrap();

        let args = serde_json::json!({ "command": "/etc/passwd" });
        assert!(policy.check("execute", &args).is_err());
    }

    #[test]
    fn deny_unmatched_when_allow_exists() {
        let policy = ScopePolicy::from_config("shell", &test_config()).unwrap();

        let args = serde_json::json!({ "command": "/home/user/random" });
        assert!(policy.check("execute", &args).is_err());
    }

    #[test]
    fn wildcard_tool_scope() {
        let policy = ScopePolicy::from_config("shell", &test_config()).unwrap();

        let args = serde_json::json!({ "path": "file.txt" });
        assert!(policy.check("read", &args).is_ok());
    }

    #[test]
    fn enforcer_with_multiple_policies() {
        let mut enforcer = ScopeEnforcer::new();

        let shell_config = ScopeConfig {
            allow: vec!["*".to_string()],
            block: vec!["execute:rm *".to_string()],
        };
        enforcer.add_policy("shell", &shell_config).unwrap();

        let docker_config = ScopeConfig {
            allow: vec!["*".to_string()],
            block: vec!["container_lifecycle:traefik".to_string()],
        };
        enforcer.add_policy("docker", &docker_config).unwrap();

        // Shell allows most things
        let args = serde_json::json!({ "command": "ls" });
        assert!(enforcer.check("shell", "execute", &args).is_ok());

        // Shell blocks rm
        let args = serde_json::json!({ "command": "rm -rf /" });
        assert!(enforcer.check("shell", "execute", &args).is_err());

        // Docker blocks traefik
        let args = serde_json::json!({ "container": "traefik" });
        assert!(enforcer.check("docker", "container_lifecycle", &args).is_err());
    }
}
