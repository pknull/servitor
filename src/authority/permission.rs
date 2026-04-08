//! Permission types and pattern matching.
//!
//! Permissions map keepers to skills using glob-style patterns.

use serde::{Deserialize, Serialize};

/// A permission grant mapping keeper to allowed skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// Name of the keeper this permission applies to.
    pub keeper: String,

    /// Skill patterns (e.g., ["shell:*", "docker:inspect_*"]).
    pub skills: Vec<String>,
}

impl Permission {
    /// Check if this permission matches the given skill.
    pub fn matches(&self, skill: &str) -> bool {
        self.skills.iter().any(|s| skill_pattern_matches(s, skill))
    }
}

/// Authorization request containing person and skill.
#[derive(Debug, Clone)]
pub struct AuthRequest {
    /// The person identity making the request.
    pub person: super::keeper::PersonId,

    /// The skill being invoked (e.g., "shell:execute").
    pub skill: String,
}

/// Result of an authorization check.
#[derive(Debug, Clone)]
pub struct AuthResult {
    /// Whether the request is allowed.
    pub allowed: bool,

    /// Name of the matched keeper, if identified.
    pub keeper: Option<String>,

    /// Human-readable reason for the decision.
    pub reason: String,
}

impl AuthResult {
    /// Create an allowed result.
    pub fn allowed(keeper: &str, reason: impl Into<String>) -> Self {
        Self {
            allowed: true,
            keeper: Some(keeper.to_string()),
            reason: reason.into(),
        }
    }

    /// Create a denied result.
    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            keeper: None,
            reason: reason.into(),
        }
    }

    /// Create a denied result for a known keeper.
    pub fn denied_keeper(keeper: &str, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            keeper: Some(keeper.to_string()),
            reason: reason.into(),
        }
    }
}

/// Glob-style pattern matching for skills.
///
/// Patterns use colon-delimited segments:
/// - `*` matches any single segment
/// - Exact match for literals
///
/// Examples:
/// - `*` matches anything
/// - `discord:*` matches `discord:123`, `discord:456`
/// - `discord:187489110150086656:*` matches `discord:187489110150086656:123`
/// - `shell:execute` matches exactly `shell:execute`
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    // Wildcard matches everything
    if pattern == "*" {
        return true;
    }

    let pattern_parts: Vec<&str> = pattern.split(':').collect();
    let value_parts: Vec<&str> = value.split(':').collect();

    // If pattern ends with *, it matches any number of remaining segments
    if pattern_parts.last() == Some(&"*") {
        let prefix_parts = &pattern_parts[..pattern_parts.len() - 1];
        if value_parts.len() < prefix_parts.len() {
            return false;
        }
        return prefix_parts
            .iter()
            .zip(value_parts.iter())
            .all(|(p, v)| *p == "*" || p == v);
    }

    // Otherwise, segment count must match
    if pattern_parts.len() != value_parts.len() {
        return false;
    }

    // Match each segment
    pattern_parts
        .iter()
        .zip(value_parts.iter())
        .all(|(p, v)| *p == "*" || p == v)
}

/// Glob-style pattern matching for skill names (supports underscore wildcards).
///
/// Examples:
/// - `docker:container_*` matches `docker:container_list`, `docker:container_inspect`
pub fn skill_pattern_matches(pattern: &str, skill: &str) -> bool {
    // First try colon-segment matching
    if pattern_matches(pattern, skill) {
        return true;
    }

    // Check for underscore wildcards within a segment
    // e.g., "docker:container_*" should match "docker:container_list"
    if pattern.contains('_') && pattern.ends_with('*') {
        let prefix = pattern.trim_end_matches('*');
        if skill.starts_with(prefix) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_wildcard() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("*", "discord:guild:channel"));
    }

    #[test]
    fn test_pattern_exact() {
        assert!(pattern_matches("egregore:local", "egregore:local"));
        assert!(!pattern_matches("egregore:local", "egregore:remote"));
        assert!(!pattern_matches("egregore:local", "discord:123"));
    }

    #[test]
    fn test_pattern_trailing_wildcard() {
        assert!(pattern_matches("discord:*", "discord:123"));
        assert!(pattern_matches("discord:*", "discord:456"));
        assert!(!pattern_matches("discord:*", "egregore:local"));

        // Trailing wildcard matches any depth
        assert!(pattern_matches(
            "discord:187489110150086656:*",
            "discord:187489110150086656:123"
        ));
        assert!(pattern_matches(
            "discord:187489110150086656:*",
            "discord:187489110150086656:456"
        ));
        assert!(!pattern_matches(
            "discord:187489110150086656:*",
            "discord:other:123"
        ));
    }

    #[test]
    fn test_pattern_segment_wildcard() {
        assert!(pattern_matches("discord:*:general", "discord:123:general"));
        assert!(!pattern_matches("discord:*:general", "discord:123:random"));
    }

    #[test]
    fn test_skill_pattern_underscore() {
        assert!(skill_pattern_matches(
            "docker:container_*",
            "docker:container_list"
        ));
        assert!(skill_pattern_matches(
            "docker:container_*",
            "docker:container_inspect"
        ));
        assert!(!skill_pattern_matches(
            "docker:container_*",
            "docker:image_list"
        ));
    }

    #[test]
    fn test_permission_matches() {
        let perm = Permission {
            keeper: "pknull".to_string(),
            skills: vec!["shell:*".to_string(), "docker:inspect_*".to_string()],
        };

        assert!(perm.matches("shell:execute"));
        assert!(perm.matches("docker:inspect_container"));
        assert!(!perm.matches("dangerous:tool"));
    }
}
