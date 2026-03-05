//! Glob pattern matching for scope enforcement.

use glob::Pattern;

use crate::error::{Result, ServitorError};

/// Compiled glob pattern for matching.
#[derive(Debug)]
pub struct ScopeMatcher {
    pattern: Pattern,
    original: String,
}

impl ScopeMatcher {
    /// Compile a glob pattern.
    pub fn new(pattern: &str) -> Result<Self> {
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
        assert_eq!(parse_scoped_pattern("execute:/etc/*"), ("execute", "/etc/*"));
        assert_eq!(parse_scoped_pattern("read:*.txt"), ("read", "*.txt"));
        assert_eq!(parse_scoped_pattern("plain-pattern"), ("*", "plain-pattern"));
    }
}
