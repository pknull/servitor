//! Argument sanitization for trace spans.
//!
//! Implements security-conscious sanitization rules from component-model.md:
//! - Redacts fields matching sensitive patterns: *key*, *secret*, *password*, *token*, *credential*, *api_key*
//! - Truncates values longer than 1KB to prevent log flooding
//! - Returns JSON string suitable for span attributes

use serde_json::{Map, Value};

/// Maximum length for a single string value before truncation.
const MAX_VALUE_LENGTH: usize = 1024;

/// Maximum nesting depth for recursive sanitization to prevent stack overflow.
const MAX_DEPTH: usize = 10;

/// Maximum array elements to process before truncating.
const MAX_ARRAY_SIZE: usize = 100;

/// Redaction marker for sensitive fields.
const REDACTED: &str = "[REDACTED]";

/// Patterns that indicate sensitive field names (case-insensitive matching).
/// NOTE: Keep aligned with egregore/src/telemetry.rs SENSITIVE_PATTERNS.
const SENSITIVE_PATTERNS: &[&str] = &[
    "key",
    "secret",
    "password",
    "token",
    "credential",
    "auth",
    "bearer",
    "session",
    "private",
    "cookie",
];

/// Sanitize tool arguments for safe inclusion in trace spans.
///
/// Applies the following transformations:
/// 1. Redacts any field whose name contains sensitive patterns
/// 2. Truncates string values longer than 1KB
/// 3. Recursively processes nested objects and arrays (with depth limit)
/// 4. Truncates arrays longer than 100 elements
///
/// # Arguments
/// * `arguments` - The raw JSON arguments from a tool call
///
/// # Returns
/// A sanitized JSON string suitable for span attributes
pub fn sanitize_arguments(arguments: &Value) -> String {
    let sanitized = sanitize_value(arguments, None, 0);
    serde_json::to_string(&sanitized).unwrap_or_else(|_| "{}".to_string())
}

/// Recursively sanitize a JSON value.
///
/// # Arguments
/// * `value` - The value to sanitize
/// * `field_name` - The field name if this value is part of an object (for sensitivity checking)
/// * `depth` - Current recursion depth (to prevent stack overflow on deeply nested structures)
fn sanitize_value(value: &Value, field_name: Option<&str>, depth: usize) -> Value {
    // Prevent stack overflow on deeply nested structures
    if depth > MAX_DEPTH {
        return Value::String("[TRUNCATED: max depth exceeded]".to_string());
    }

    // Check if this field should be redacted based on its name
    if let Some(name) = field_name {
        if is_sensitive_field(name) {
            return Value::String(REDACTED.to_string());
        }
    }

    match value {
        Value::Object(map) => {
            let sanitized: Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sanitize_value(v, Some(k), depth + 1)))
                .collect();
            Value::Object(sanitized)
        }
        Value::Array(arr) => {
            let total_len = arr.len();
            let truncated = total_len > MAX_ARRAY_SIZE;
            let items_to_process = if truncated { MAX_ARRAY_SIZE } else { total_len };

            let mut sanitized: Vec<Value> = arr
                .iter()
                .take(items_to_process)
                .map(|v| sanitize_value(v, None, depth + 1))
                .collect();

            if truncated {
                sanitized.push(Value::String(format!(
                    "[TRUNCATED: {} of {} items shown]",
                    items_to_process, total_len
                )));
            }
            Value::Array(sanitized)
        }
        Value::String(s) => {
            if s.len() > MAX_VALUE_LENGTH {
                // Find safe UTF-8 boundary for truncation
                let mut end = MAX_VALUE_LENGTH;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                let truncated = format!("{}... [truncated, {} bytes total]", &s[..end], s.len());
                Value::String(truncated)
            } else {
                value.clone()
            }
        }
        // Numbers, booleans, and nulls pass through unchanged
        _ => value.clone(),
    }
}

/// Maximum length for tool result content before truncation.
/// Tool results can be quite large; truncate to prevent log flooding.
const MAX_TOOL_RESULT_LENGTH: usize = 8192;

/// Patterns that indicate sensitive data in tool output (all lowercase for matching).
const SENSITIVE_VALUE_PATTERNS: &[&str] = &[
    "password=",
    "password:",
    "token=",
    "token:",
    "secret=",
    "secret:",
    "api_key=",
    "apikey=",
    "bearer ",
    "authorization:",
    "-----begin",
    "-----end",
];

/// Sanitize tool result content for safe inclusion in logs and LLM context.
///
/// Applies the following transformations:
/// 1. Truncates content longer than 8KB
/// 2. Redacts lines containing sensitive patterns
/// 3. Redacts entire PEM blocks (private keys, certificates)
///
/// # Arguments
/// * `content` - The raw tool result content
///
/// # Returns
/// A sanitized string safe for logging
pub fn sanitize_tool_result(content: &str) -> String {
    let lower = content.to_lowercase();

    // Check for PEM-encoded content (private keys, certificates) - redact entire content
    if lower.contains("-----begin") && lower.contains("-----end") {
        return "[REDACTED: PEM-encoded sensitive content]".to_string();
    }

    // Check for other sensitive patterns and redact affected lines
    let mut result = content.to_string();
    let mut needs_check = true;

    while needs_check {
        needs_check = false;
        let current_lower = result.to_lowercase();

        for pattern in SENSITIVE_VALUE_PATTERNS {
            if current_lower.contains(pattern) {
                result = result
                    .lines()
                    .map(|line| {
                        if line.to_lowercase().contains(pattern) {
                            "[REDACTED: sensitive content]"
                        } else {
                            line
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                needs_check = true; // Recheck in case redaction revealed more patterns
                break;
            }
        }
    }

    // Then truncate if too long
    if result.len() > MAX_TOOL_RESULT_LENGTH {
        let mut end = MAX_TOOL_RESULT_LENGTH;
        while end > 0 && !result.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}... [truncated, {} bytes total]",
            &result[..end],
            content.len()
        )
    } else {
        result
    }
}

/// Check if a field name matches any sensitive patterns.
///
/// Uses case-insensitive substring matching.
fn is_sensitive_field(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_redacts_api_key_field() {
        let args = json!({
            "command": "curl",
            "api_key": "sk-secret-12345"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["command"], "curl");
        assert_eq!(parsed["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_password_field() {
        let args = json!({
            "username": "admin",
            "password": "hunter2"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["username"], "admin");
        assert_eq!(parsed["password"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_token_field() {
        let args = json!({
            "auth_token": "bearer-xyz",
            "access_token": "oauth-abc"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["auth_token"], "[REDACTED]");
        assert_eq!(parsed["access_token"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_secret_field() {
        let args = json!({
            "client_secret": "shh",
            "secret_key": "double-secret"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["client_secret"], "[REDACTED]");
        assert_eq!(parsed["secret_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_credential_field() {
        let args = json!({
            "credentials": "user:pass",
            "db_credential": "postgres://..."
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["credentials"], "[REDACTED]");
        assert_eq!(parsed["db_credential"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_key_variations() {
        let args = json!({
            "apikey": "key1",
            "API_KEY": "key2",
            "openai_key": "key3",
            "private_key": "key4"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["apikey"], "[REDACTED]");
        assert_eq!(parsed["API_KEY"], "[REDACTED]");
        assert_eq!(parsed["openai_key"], "[REDACTED]");
        assert_eq!(parsed["private_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_truncates_long_strings() {
        let long_content = "x".repeat(2000);
        let args = json!({
            "content": long_content
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let content = parsed["content"].as_str().unwrap();
        assert!(content.contains("[truncated, 2000 bytes total]"));
        assert!(content.len() < 1200); // Should be around 1024 + marker
    }

    #[test]
    fn sanitize_handles_nested_objects() {
        let args = json!({
            "config": {
                "url": "https://example.com",
                "api_key": "secret-nested"
            }
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["config"]["url"], "https://example.com");
        assert_eq!(parsed["config"]["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_handles_arrays() {
        let args = json!({
            "items": ["safe", "also-safe"],
            "tokens": ["should-not-be-redacted"]
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        // Array values themselves are not redacted - only the field "tokens" would be
        assert_eq!(parsed["tokens"], "[REDACTED]");
        assert_eq!(parsed["items"][0], "safe");
    }

    #[test]
    fn sanitize_preserves_numbers_and_booleans() {
        let args = json!({
            "count": 42,
            "enabled": true,
            "ratio": 1.5
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["count"], 42);
        assert_eq!(parsed["enabled"], true);
        assert_eq!(parsed["ratio"], 1.5);
    }

    #[test]
    fn sanitize_handles_null() {
        let args = json!({
            "optional": null
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert!(parsed["optional"].is_null());
    }

    #[test]
    fn sanitize_handles_empty_object() {
        let args = json!({});
        let result = sanitize_arguments(&args);
        assert_eq!(result, "{}");
    }

    #[test]
    fn sanitize_case_insensitive_matching() {
        let args = json!({
            "API_KEY": "upper",
            "Api_Key": "mixed",
            "api_key": "lower"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["API_KEY"], "[REDACTED]");
        assert_eq!(parsed["Api_Key"], "[REDACTED]");
        assert_eq!(parsed["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_truncates_utf8_safely() {
        // Each emoji is 4 bytes, so 500 emojis = 2000 bytes (over 1KB limit)
        let emoji_string = "🎉".repeat(500);
        let args = json!({
            "content": emoji_string
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let content = parsed["content"].as_str().unwrap();
        assert!(content.contains("[truncated,"));
        // Verify the string is valid UTF-8 by attempting to use it
        let _ = content.to_string();
    }

    #[test]
    fn sanitize_truncates_deeply_nested_structures() {
        // Create a deeply nested structure (> MAX_DEPTH levels)
        let mut nested = json!({ "value": "deep" });
        for _ in 0..15 {
            nested = json!({ "nested": nested });
        }

        let result = sanitize_arguments(&nested);
        assert!(result.contains("[TRUNCATED: max depth exceeded]"));
    }

    #[test]
    fn sanitize_truncates_large_arrays() {
        // Create an array with > MAX_ARRAY_SIZE elements
        let large_array: Vec<i32> = (0..150).collect();
        let args = json!({
            "items": large_array
        });

        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let items = parsed["items"].as_array().unwrap();
        // Should have 100 elements + 1 truncation marker
        assert_eq!(items.len(), 101);
        assert!(items
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("[TRUNCATED:"));
        assert!(items
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("100 of 150"));
    }

    #[test]
    fn sanitize_redacts_cookie_field() {
        let args = json!({
            "session_cookie": "abc123",
            "auth_cookie": "xyz789"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["session_cookie"], "[REDACTED]");
        assert_eq!(parsed["auth_cookie"], "[REDACTED]");
    }

    // Tests for sanitize_tool_result

    #[test]
    fn tool_result_redacts_password() {
        let content = "Config loaded:\nuser=admin\npassword=secret123\nhost=localhost";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: sensitive content]"));
        assert!(!result.contains("secret123"));
        assert!(result.contains("user=admin"));
    }

    #[test]
    fn tool_result_redacts_token() {
        let content = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: sensitive content]"));
        assert!(!result.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    }

    #[test]
    fn tool_result_redacts_private_key() {
        let content =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: PEM-encoded sensitive content]"));
        assert!(!result.contains("MIIEowIBAAKCAQEA"));
    }

    #[test]
    fn tool_result_truncates_long_content() {
        let long_content = "x".repeat(10000);
        let result = sanitize_tool_result(&long_content);
        assert!(result.contains("[truncated, 10000 bytes total]"));
        assert!(result.len() < 9000);
    }

    #[test]
    fn tool_result_preserves_safe_content() {
        let content = "Operation completed successfully.\nFiles processed: 42\nStatus: OK";
        let result = sanitize_tool_result(content);
        assert_eq!(result, content);
    }
}
