//! Scope enforcement policy — allow/block logic.

use std::fmt;

use crate::config::ScopeConfig;
use crate::egregore::messages::TaskScopeOverride;
use crate::error::{LocalDiagnostic, Result, ServitorError};
use crate::scope::matcher::{parse_scoped_pattern, ScopeMatcher};

/// The scope rule behind a denial.
///
/// Names the rule by source, list and position, never by the arguments it
/// matched, so it can be reported to the requester. Policy rules index the
/// server's configured `scope.allow` / `scope.block` lists, which stay local
/// to the executor; override rules index the task's own `scope_override`
/// lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRule {
    /// A block rule matched one of the call's targets.
    Blocked { source: RuleSource, index: usize },
    /// Allow rules exist and none matched.
    NotAllowed { source: RuleSource },
    /// A task scope override rule is malformed.
    InvalidOverride { list: RuleList, index: usize },
}

/// Where a scope rule comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    /// The server's configured scope policy.
    Policy,
    /// The task's `scope_override`.
    TaskOverride,
}

/// Which list of a scope a rule sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleList {
    Allow,
    Block,
}

impl fmt::Display for ScopeRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = |source: &RuleSource| match source {
            RuleSource::Policy => "policy",
            RuleSource::TaskOverride => "task scope override",
        };
        match self {
            Self::Blocked {
                source: from,
                index,
            } => {
                write!(f, "blocked by {} block rule {index}", source(from))
            }
            Self::NotAllowed { source: from } => {
                write!(f, "not permitted by any {} allow rule", source(from))
            }
            Self::InvalidOverride { list, index } => {
                let list = match list {
                    RuleList::Allow => "allow",
                    RuleList::Block => "block",
                };
                write!(
                    f,
                    "refused: task scope override {list} rule {index} is invalid"
                )
            }
        }
    }
}

fn violation(rule: ScopeRule, diagnostic: &str) -> ServitorError {
    ServitorError::ScopeViolation {
        rule,
        diagnostic: LocalDiagnostic::new(diagnostic),
    }
}

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
        Ok(Self {
            server_name: server_name.to_string(),
            allow: compile_patterns(&config.allow)?,
            block: compile_patterns(&config.block)?,
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
        self.check_with_override(tool_name, args, None)
    }

    /// Check if a tool call is allowed with an optional task-level override.
    ///
    /// A denial is a [`ServitorError::ScopeViolation`] carrying the
    /// [`ScopeRule`] that refused the call.
    pub fn check_with_override(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        scope_override: Option<&TaskScopeOverride>,
    ) -> Result<()> {
        // Extract key arguments for matching
        let targets = extract_targets(tool_name, args);

        // Check block patterns first (they take precedence)
        if let Some((index, target, pattern)) = matching_block(&self.block, &targets, |scope| {
            scope_matches(scope, tool_name)
        }) {
            return Err(violation(
                ScopeRule::Blocked {
                    source: RuleSource::Policy,
                    index,
                },
                &format!("blocked by policy: {target} matches block pattern '{pattern}'"),
            ));
        }

        // If no allow patterns, permit by default
        if !self.allow.is_empty() && !matches_allow_patterns(&self.allow, tool_name, &targets) {
            return Err(violation(
                ScopeRule::NotAllowed {
                    source: RuleSource::Policy,
                },
                &format!(
                    "not allowed: tool '{}' with targets {:?} not permitted by any allow pattern",
                    tool_name, targets
                ),
            ));
        }

        let Some(scope_override) = scope_override else {
            return Ok(());
        };

        if scope_override.is_empty() {
            return Ok(());
        }

        let override_block =
            compile_task_override_patterns(&scope_override.block, RuleList::Block)?;
        let full_scope = format!("{}:{}", self.server_name, tool_name);
        if let Some((index, target, pattern)) = matching_block(&override_block, &targets, |scope| {
            override_scope_matches(scope, &full_scope)
        }) {
            return Err(violation(
                ScopeRule::Blocked {
                    source: RuleSource::TaskOverride,
                    index,
                },
                &format!(
                    "blocked by task scope override: {target} matches block pattern '{pattern}'"
                ),
            ));
        }

        if !scope_override.allow.is_empty() {
            let override_allow =
                compile_task_override_patterns(&scope_override.allow, RuleList::Allow)?;
            if !matches_override_allow_patterns(&override_allow, &full_scope, &targets) {
                return Err(violation(
                    ScopeRule::NotAllowed {
                        source: RuleSource::TaskOverride,
                    },
                    &format!(
                        "task scope override does not allow tool '{}' with targets {:?}",
                        tool_name, targets
                    ),
                ));
            }
        }

        Ok(())
    }
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<(String, ScopeMatcher)>> {
    let mut compiled = Vec::new();
    for pattern in patterns {
        let (scope, pat) = parse_scoped_pattern(pattern);
        compiled.push((scope.to_string(), ScopeMatcher::new(pat)?));
    }
    Ok(compiled)
}

/// Compile a task's override rules. A malformed rule refuses the call as
/// [`ScopeRule::InvalidOverride`], naming its position rather than its text.
fn compile_task_override_patterns(
    patterns: &[String],
    list: RuleList,
) -> Result<Vec<(String, ScopeMatcher)>> {
    let mut compiled = Vec::new();
    for (index, pattern) in patterns.iter().enumerate() {
        let rule = parse_task_override_pattern(pattern)
            .and_then(|(scope, pat)| Ok((scope, ScopeMatcher::new(&pat)?)));
        match rule {
            Ok(rule) => compiled.push(rule),
            Err(error) => {
                return Err(violation(
                    ScopeRule::InvalidOverride { list, index },
                    &error.to_string(),
                ))
            }
        }
    }
    Ok(compiled)
}

fn parse_task_override_pattern(pattern: &str) -> Result<(String, String)> {
    let mut segments = pattern.splitn(3, ':');
    let server = segments.next().unwrap_or_default();
    let tool = segments.next().unwrap_or_default();
    let target = segments.next().unwrap_or("*");

    if server.is_empty() || tool.is_empty() {
        return Err(ServitorError::Config {
            reason: format!(
                "invalid task scope override pattern '{}': expected '<server>:<tool>:<target>'",
                pattern
            ),
        });
    }

    Ok((format!("{}:{}", server, tool), target.to_string()))
}

/// The first block rule, in list order, whose scope applies and whose
/// pattern matches a target: its index, the target, and the rule's text.
fn matching_block<'a>(
    patterns: &'a [(String, ScopeMatcher)],
    targets: &'a [String],
    applies: impl Fn(&str) -> bool,
) -> Option<(usize, &'a str, String)> {
    patterns
        .iter()
        .enumerate()
        .filter(|(_, (scope, _))| applies(scope))
        .find_map(|(index, (scope, matcher))| {
            targets
                .iter()
                .find(|target| matcher.matches(target))
                .map(|target| {
                    (
                        index,
                        target.as_str(),
                        format!("{scope}:{}", matcher.pattern()),
                    )
                })
        })
}

fn matches_allow_patterns(
    patterns: &[(String, ScopeMatcher)],
    tool_name: &str,
    targets: &[String],
) -> bool {
    for (scope, matcher) in patterns {
        if scope_matches(scope, tool_name) {
            if matcher.pattern() == "*" {
                return true;
            }
            for target in targets {
                if matcher.matches(target) {
                    return true;
                }
            }
        }
    }

    false
}

fn matches_override_allow_patterns(
    patterns: &[(String, ScopeMatcher)],
    full_scope: &str,
    targets: &[String],
) -> bool {
    for (scope, matcher) in patterns {
        if override_scope_matches(scope, full_scope) {
            if matcher.pattern() == "*" {
                return true;
            }
            for target in targets {
                if matcher.matches(target) {
                    return true;
                }
            }
        }
    }

    false
}

fn override_scope_matches(scope: &str, full_scope: &str) -> bool {
    scope == "*" || scope == full_scope
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
        scope_override: Option<&TaskScopeOverride>,
    ) -> Result<()> {
        if let Some(policy) = self.policies.get(mcp_name) {
            policy.check_with_override(tool_name, args, scope_override)
        } else {
            if let Some(scope_override) = scope_override {
                let policy = ScopePolicy::from_config(
                    mcp_name,
                    &ScopeConfig {
                        allow: vec![],
                        block: vec![],
                    },
                )?;
                policy.check_with_override(tool_name, args, Some(scope_override))
            } else {
                // No policy defined, allow by default
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ScopeConfig {
        ScopeConfig {
            allow: vec!["execute:~/scripts/*".to_string(), "*:*.txt".to_string()],
            block: vec!["execute:/etc/*".to_string(), "execute:rm *".to_string()],
        }
    }

    fn allow_read_only_override() -> TaskScopeOverride {
        TaskScopeOverride {
            allow: vec!["shell:read:*".to_string()],
            block: vec![],
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
    fn scope_override_can_further_restrict_allowed_tools() {
        let policy = ScopePolicy::from_config(
            "shell",
            &ScopeConfig {
                allow: vec!["*".to_string()],
                block: vec![],
            },
        )
        .unwrap();

        let args = serde_json::json!({ "path": "notes.txt" });
        assert!(policy
            .check_with_override("read", &args, Some(&allow_read_only_override()))
            .is_ok());
        assert!(policy
            .check_with_override(
                "execute",
                &serde_json::json!({ "command": "ls" }),
                Some(&allow_read_only_override())
            )
            .is_err());
    }

    #[test]
    fn scope_override_cannot_expand_base_policy() {
        let policy = ScopePolicy::from_config(
            "shell",
            &ScopeConfig {
                allow: vec!["read:*".to_string()],
                block: vec![],
            },
        )
        .unwrap();

        let scope_override = TaskScopeOverride {
            allow: vec!["shell:execute:*".to_string()],
            block: vec![],
        };

        let args = serde_json::json!({ "command": "ls" });
        assert!(policy
            .check_with_override("execute", &args, Some(&scope_override))
            .is_err());
    }

    #[test]
    fn scope_override_block_takes_precedence() {
        let policy = ScopePolicy::from_config(
            "shell",
            &ScopeConfig {
                allow: vec!["*".to_string()],
                block: vec![],
            },
        )
        .unwrap();

        let scope_override = TaskScopeOverride {
            allow: vec![],
            block: vec!["shell:execute:rm *".to_string()],
        };

        let args = serde_json::json!({ "command": "rm -rf /tmp/demo" });
        assert!(policy
            .check_with_override("execute", &args, Some(&scope_override))
            .is_err());
    }

    #[test]
    fn invalid_task_scope_override_pattern_is_rejected() {
        let policy = ScopePolicy::from_config(
            "shell",
            &ScopeConfig {
                allow: vec!["*".to_string()],
                block: vec![],
            },
        )
        .unwrap();

        let scope_override = TaskScopeOverride {
            allow: vec!["shell:read:*".to_string(), "shell".to_string()],
            block: vec![],
        };

        let args = serde_json::json!({ "command": "ls" });
        let error = policy
            .check_with_override("execute", &args, Some(&scope_override))
            .unwrap_err();
        assert_eq!(
            rule_of(error),
            ScopeRule::InvalidOverride {
                list: RuleList::Allow,
                index: 1,
            }
        );
    }

    fn rule_of(error: ServitorError) -> ScopeRule {
        match error {
            ServitorError::ScopeViolation { rule, .. } => rule,
            other => panic!("expected a scope violation, got {other}"),
        }
    }

    #[test]
    fn denials_name_the_rule_by_list_and_position() {
        let policy = ScopePolicy::from_config("shell", &test_config()).unwrap();
        let check = |args: serde_json::Value, scope_override: Option<&TaskScopeOverride>| {
            rule_of(
                policy
                    .check_with_override("execute", &args, scope_override)
                    .unwrap_err(),
            )
        };

        assert_eq!(
            check(serde_json::json!({ "command": "rm -rf /" }), None),
            ScopeRule::Blocked {
                source: RuleSource::Policy,
                index: 1,
            }
        );
        assert_eq!(
            check(serde_json::json!({ "command": "/home/user/random" }), None),
            ScopeRule::NotAllowed {
                source: RuleSource::Policy,
            }
        );

        let scope_override = TaskScopeOverride {
            allow: vec![],
            block: vec![
                "shell:read:*".to_string(),
                "shell:execute:~/scripts/b*".to_string(),
            ],
        };
        assert_eq!(
            check(
                serde_json::json!({ "command": "~/scripts/backup.sh" }),
                Some(&scope_override)
            ),
            ScopeRule::Blocked {
                source: RuleSource::TaskOverride,
                index: 1,
            }
        );
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
        assert!(enforcer.check("shell", "execute", &args, None).is_ok());

        // Shell blocks rm
        let args = serde_json::json!({ "command": "rm -rf /" });
        assert!(enforcer.check("shell", "execute", &args, None).is_err());

        // Docker blocks traefik
        let args = serde_json::json!({ "container": "traefik" });
        assert!(enforcer
            .check("docker", "container_lifecycle", &args, None)
            .is_err());
    }
}
