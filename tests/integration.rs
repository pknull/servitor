//! Integration tests for Servitor.

use servitor::config::Config;
use servitor::egregore::messages::{Task, TaskScopeOverride, TaskStatus};
use servitor::identity::Identity;
use servitor::mcp::McpPool;
use servitor::scope::ScopeEnforcer;

/// Test configuration parsing and validation.
#[test]
fn config_roundtrip() {
    let toml = r#"
[identity]
data_dir = "/tmp/servitor-test"

[egregore]
api_url = "http://127.0.0.1:7654"

[mcp.test]
transport = "stdio"
command = "echo"
scope.allow = ["*"]

[agent]
timeout_secs = 60
"#;

    let config = Config::from_str(toml).unwrap();
    assert_eq!(config.mcp.len(), 1);
    assert!(config.mcp.contains_key("test"));
    assert_eq!(config.task.offer_ttl_secs, 300);
}

/// Test identity generation and signing.
#[test]
fn identity_sign_verify() {
    let identity = Identity::generate();
    let message = b"test message for signing";

    let signature = identity.sign(message);
    let public_id = identity.public_id();

    // Verify signature
    let valid = public_id.verify(message, &signature).unwrap();
    assert!(valid, "signature should be valid");

    // Wrong message should fail
    let wrong = public_id.verify(b"wrong message", &signature).unwrap();
    assert!(!wrong, "wrong message should fail verification");
}

/// Test scope enforcement logic.
#[test]
fn scope_enforcement() {
    let mut enforcer = ScopeEnforcer::new();

    // Add a policy that allows scripts but blocks system paths
    let config = servitor::config::ScopeConfig {
        allow: vec!["*".to_string()],
        block: vec!["execute:/etc/*".to_string(), "execute:rm *".to_string()],
    };
    enforcer.add_policy("shell", &config).unwrap();

    // Allowed command
    let args = serde_json::json!({ "command": "ls ~/Documents" });
    assert!(enforcer.check("shell", "execute", &args, None).is_ok());

    // Blocked command
    let args = serde_json::json!({ "command": "/etc/passwd" });
    assert!(enforcer.check("shell", "execute", &args, None).is_err());

    // Blocked rm
    let args = serde_json::json!({ "command": "rm -rf /" });
    assert!(enforcer.check("shell", "execute", &args, None).is_err());
}

/// Test MCP pool creation (without actual servers).
#[test]
fn mcp_pool_creation() {
    let pool = McpPool::new();
    assert!(pool.capabilities().is_empty());
    assert!(pool.all_tools().is_empty());
}

/// Test task message construction.
#[test]
fn task_message_construction() {
    let task = Task {
        msg_type: "task".to_string(),
        id: Some("task-abc".to_string()),
        hash: "abc123".to_string(),
        task_type: Some("inventory:count".to_string()),
        request: Some("Count items in the pantry".to_string()),
        requestor: None,
        prompt: "Count items in the pantry".to_string(),
        required_caps: vec!["inventory".to_string()],
        parent_id: None,
        context: std::collections::HashMap::new(),
        scope_override: Some(TaskScopeOverride {
            allow: vec!["inventory:read:*".to_string()],
            block: vec!["inventory:write:*".to_string()],
        }),
        priority: 0,
        timeout_secs: Some(60),
        author: None,
        keeper: None,
        tool_calls: vec![],
        depends_on: vec![],
    };

    let json = serde_json::to_string(&task).unwrap();
    assert!(json.contains("task"));
    assert!(json.contains("Count items"));

    // Roundtrip
    let parsed: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.prompt, task.prompt);
    assert_eq!(
        parsed.scope_override.unwrap().block,
        vec!["inventory:write:*".to_string()]
    );
}

/// Test result-hash stability and TaskResult shape after the 0.3.0 Attestation
/// removal. Servitor no longer signs results separately — feed-level signing
/// comes from the local Egregore node's message envelope.
#[test]
fn task_result_shape() {
    use servitor::egregore::messages::TaskResult;

    let identity = Identity::generate();
    let result_hash = "deadbeef1234567890";

    let task_result = TaskResult {
        msg_type: "task_result".to_string(),
        task_id: "task-abc".to_string(),
        servitor: identity.public_id(),
        correlation_id: "corr-123".to_string(),
        task_hash: "task-abc".to_string(),
        result_hash: result_hash.to_string(),
        status: TaskStatus::Success,
        result: Some(serde_json::json!({ "answer": 42 })),
        error: None,
        duration_seconds: Some(1),
        trace_id: None,
    };

    // Serialize and check structure
    let json = serde_json::to_string_pretty(&task_result).unwrap();
    assert!(json.contains("task_result"));
    assert!(json.contains("result_hash"));
    // Attestation fields must NOT appear in the post-0.3.0 wire format.
    assert!(!json.contains("attestation"));
    assert!(!json.contains("\"signature\""));
}

/// Test trace linkage on task results.
#[test]
fn task_result_trace_id_roundtrip() {
    use servitor::egregore::messages::TaskResult;

    let identity = Identity::generate();
    let result = TaskResult {
        msg_type: "task_result".to_string(),
        task_id: "task-trace".to_string(),
        servitor: identity.public_id(),
        correlation_id: "corr-trace".to_string(),
        task_hash: "task-trace".to_string(),
        result_hash: "trace-result-hash".to_string(),
        status: TaskStatus::Success,
        result: Some(serde_json::json!({ "text": "ok" })),
        error: None,
        duration_seconds: Some(1),
        trace_id: Some("trace-123".to_string()),
    };

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["trace_id"], "trace-123");

    let parsed: TaskResult = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.trace_id.as_deref(), Some("trace-123"));
}

/// Test trace span serialization shape.
#[test]
fn trace_span_serialization() {
    use chrono::Utc;
    use servitor::egregore::messages::{TraceEvent, TraceSpan, TraceSpanStatus};
    use std::collections::HashMap;

    let mut span = TraceSpan::new(
        "trace-123",
        "span-abc",
        Some("parent-xyz".to_string()),
        "task_execution",
        "@servitor.ed25519",
        Utc::now(),
        Utc::now(),
        TraceSpanStatus::Ok,
    );
    span.attributes
        .insert("task_id".to_string(), serde_json::json!("task-123"));
    span.events.push(TraceEvent {
        ts: Utc::now(),
        name: "image_pulled".to_string(),
        attributes: HashMap::new(),
    });

    let json = serde_json::to_value(&span).unwrap();
    assert_eq!(json["type"], "trace_span");
    assert_eq!(json["trace_id"], "trace-123");
    assert_eq!(json["span_id"], "span-abc");
    assert_eq!(json["parent_span_id"], "parent-xyz");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["attributes"]["task_id"], "task-123");
    assert_eq!(json["events"][0]["name"], "image_pulled");
}

// LLM-specific tests removed: provider_capabilities, agent_context
// Servitors are now pure tool executors — no LLM provider or conversation context.
