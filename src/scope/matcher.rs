//! Glob pattern matching for scope enforcement.

use glob::Pattern;

use crate::error::{Result, ServitorError};

/// Maximum allowed length for a glob pattern.
const MAX_PATTERN_LENGTH: usize = 256;

/// Maximum consecutive wildcards allowed.
const MAX_CONSECUTIVE_WILDCARDS: usize = 3;

/// Compiled glob pattern for matching.
#[derive(Debug)]
pub struct ScopeMatcher {
    pattern: Pattern,
    original: String,
}

impl ScopeMatcher {
    /// Compile a glob pattern.
    ///
    /// Validates pattern complexity to prevent pathological patterns:
    /// - Maximum length of 256 characters
    /// - Maximum 3 consecutive wildcards
    pub fn new(pattern: &str) -> Result<Self> {
        // Validate pattern length
        if pattern.len() > MAX_PATTERN_LENGTH {
            return Err(ServitorError::Config {
                reason: format!(
                    "glob pattern too long ({} chars, max {}): '{}'",
                    pattern.len(),
                    MAX_PATTERN_LENGTH,
                    &pattern[..50]
                ),
            });
        }

        // Check for excessive consecutive wildcards
        let consecutive_wildcards = count_max_consecutive_wildcards(pattern);
        if consecutive_wildcards > MAX_CONSECUTIVE_WILDCARDS {
            return Err(ServitorError::Config {
                reason: format!(
                    "glob pattern has {} consecutive wildcards (max {}): '{}'",
                    consecutive_wildcards, MAX_CONSECUTIVE_WILDCARDS, pattern
                ),
            });
        }

        let compiled = Pattern::new(pattern).map_err(|e| ServitorError::Config {
            reason: format!("invalid glob pattern '{}': {}", pattern, e),
        })?;

        Ok(Self {
            pattern: compiled,
            original: pattern.to_string(),
        })
    }

    /// Check if a string matches the pattern.
    pub fn matches(&self, s: &str) -> bool {
        self.pattern.matches(s)
    }

    /// Get the original pattern string.
    pub fn pattern(&self) -> &str {
        &self.original
    }
}

/// Parse a scoped pattern like "execute:/etc/*" into (scope, pattern).
pub fn parse_scoped_pattern(pattern: &str) -> (&str, &str) {
    if let Some(idx) = pattern.find(':') {
        (&pattern[..idx], &pattern[idx + 1..])
    } else {
        // No scope prefix, treat entire string as pattern
        ("*", pattern)
    }
}

/// Count the maximum consecutive wildcard segments in a pattern.
///
/// A wildcard segment is `*` or `**`. Segments are considered consecutive
/// if separated only by `/`. For example:
/// - `*/*/*` has 3 consecutive wildcard segments
/// - `/home/**/*.rs` has 2 consecutive wildcard segments
fn count_max_consecutive_wildcards(pattern: &str) -> usize {
    let segments: Vec<&str> = pattern.split('/').collect();
    let mut max_consecutive = 0;
    let mut current_consecutive = 0;

    for segment in segments {
        // A segment is a wildcard if it's just * or **
        let is_wildcard = segment == "*" || segment == "**";

        if is_wildcard {
            current_consecutive += 1;
            max_consecutive = max_consecutive.max(current_consecutive);
        } else if segment.contains('*') {
            // Contains wildcard but not a pure wildcard segment (e.g., "*.rs")
            // This counts as a wildcard segment too
            current_consecutive += 1;
            max_consecutive = max_consecutive.max(current_consecutive);
        } else {
            // Non-wildcard segment, reset counter
            current_consecutive = 0;
        }
    }

    max_consecutive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_glob_matching() {
        let matcher = ScopeMatcher::new("/home/user/*").unwrap();
        assert!(matcher.matches("/home/user/file.txt"));
        assert!(matcher.matches("/home/user/subdir"));
        assert!(!matcher.matches("/home/other/file.txt"));
    }

    #[test]
    fn recursive_glob() {
        let matcher = ScopeMatcher::new("/home/**/*.rs").unwrap();
        assert!(matcher.matches("/home/project/src/main.rs"));
        assert!(matcher.matches("/home/a/b/c/d.rs"));
        assert!(!matcher.matches("/home/project/main.txt"));
    }

    #[test]
    fn wildcard_all() {
        let matcher = ScopeMatcher::new("*").unwrap();
        assert!(matcher.matches("anything"));
        assert!(matcher.matches(""));
    }

    #[test]
    fn parse_scoped() {
        assert_eq!(
            parse_scoped_pattern("execute:/etc/*"),
            ("execute", "/etc/*")
        );
        assert_eq!(parse_scoped_pattern("read:*.txt"), ("read", "*.txt"));
        assert_eq!(
            parse_scoped_pattern("plain-pattern"),
            ("*", "plain-pattern")
        );
    }

    #[test]
    fn rejects_pattern_too_long() {
        let long_pattern = "a".repeat(300);
        let result = ScopeMatcher::new(&long_pattern);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too long"));
    }

    #[test]
    fn rejects_excessive_wildcards() {
        // Four consecutive wildcards (more than allowed 3)
        let result = ScopeMatcher::new("/home/*/*/*/*/file");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("consecutive wildcards"));
    }

    #[test]
    fn allows_reasonable_wildcards() {
        // Three consecutive wildcards is allowed
        let result = ScopeMatcher::new("/home/*/*/*/file");
        assert!(result.is_ok());

        // ** counts as one wildcard unit
        let result = ScopeMatcher::new("/home/**/*.rs");
        assert!(result.is_ok());
    }

    #[test]
    fn consecutive_wildcard_counting() {
        assert_eq!(count_max_consecutive_wildcards("*"), 1);
        assert_eq!(count_max_consecutive_wildcards("**"), 1); // ** is one segment
        assert_eq!(count_max_consecutive_wildcards("*/*"), 2);
        assert_eq!(count_max_consecutive_wildcards("*/*/*"), 3);
        assert_eq!(count_max_consecutive_wildcards("*/*/*/*"), 4);
        assert_eq!(count_max_consecutive_wildcards("/home/**/*.rs"), 2); // ** and *.rs
        assert_eq!(count_max_consecutive_wildcards("no-wildcards"), 0);
        // Non-consecutive wildcards
        assert_eq!(count_max_consecutive_wildcards("/*/foo/*"), 1); // each is separated by "foo"
        assert_eq!(count_max_consecutive_wildcards("*.txt"), 1);
    }
}
