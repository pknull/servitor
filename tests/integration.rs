//! Integration tests for Servitor.

use servitor::config::Config;
use servitor::egregore::messages::{Task, TaskStatus};
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

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

[mcp.test]
transport = "stdio"
command = "echo"
scope.allow = ["*"]

[agent]
max_turns = 10
timeout_secs = 60
"#;

    let config = Config::from_str(toml).unwrap();
    assert_eq!(config.llm.provider, "anthropic");
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
    assert!(enforcer.check("shell", "execute", &args).is_ok());

    // Blocked command
    let args = serde_json::json!({ "command": "/etc/passwd" });
    assert!(enforcer.check("shell", "execute", &args).is_err());

    // Blocked rm
    let args = serde_json::json!({ "command": "rm -rf /" });
    assert!(enforcer.check("shell", "execute", &args).is_err());
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
        priority: 0,
        timeout_secs: Some(60),
        author: None,
        keeper: None,
    };

    let json = serde_json::to_string(&task).unwrap();
    assert!(json.contains("task"));
    assert!(json.contains("Count items"));

    // Roundtrip
    let parsed: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.prompt, task.prompt);
}

/// Test attestation signing in task results.
#[test]
fn attestation_signing() {
    use chrono::Utc;
    use servitor::egregore::messages::{Attestation, TaskResult};

    let identity = Identity::generate();
    let result_hash = "deadbeef1234567890";

    let signature = identity.sign_hash(result_hash);
    let attestation = Attestation {
        servitor_id: identity.public_id(),
        signature: signature.clone(),
        timestamp: Utc::now(),
    };

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
        attestation,
    };

    // Verify the signature
    let valid = task_result
        .attestation
        .servitor_id
        .verify(result_hash.as_bytes(), &signature)
        .unwrap();
    assert!(valid);

    // Serialize and check structure
    let json = serde_json::to_string_pretty(&task_result).unwrap();
    assert!(json.contains("task_result"));
    assert!(json.contains("attestation"));
    assert!(json.contains("signature"));
}

/// Test provider capabilities structure.
#[test]
fn provider_capabilities() {
    use servitor::agent::provider::ProviderCapabilities;

    let caps = ProviderCapabilities {
        supports_tools: true,
        supports_vision: true,
        supports_streaming: false,
        max_tokens: Some(4096),
    };

    assert!(caps.supports_tools);
    assert_eq!(caps.max_tokens, Some(4096));
}

#[test]
fn capability_challenge_roundtrip() {
    use chrono::Utc;
    use servitor::egregore::messages::{CapabilityChallenge, CapabilityProof};

    let challenge = CapabilityChallenge {
        msg_type: "capability_challenge".to_string(),
        challenge_id: "challenge-1".to_string(),
        task_id: "task-1".to_string(),
        servitor: Identity::generate().public_id(),
        capability: "shell:execute".to_string(),
        challenger: None,
        ttl_seconds: 30,
        timestamp: Utc::now(),
    };

    let json = serde_json::to_value(&challenge).unwrap();
    let parsed: CapabilityChallenge = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.capability, "shell:execute");

    let proof_json = serde_json::json!({
        "type": "capability_proof",
        "challenge_id": "challenge-1",
        "task_id": "task-1",
        "servitor": challenge.servitor,
        "capability": "shell:execute",
        "verified": true,
        "matched_tools": ["shell_execute"],
        "attestation": {
            "servitor_id": Identity::generate().public_id(),
            "signature": "sig",
            "timestamp": Utc::now()
        },
        "timestamp": Utc::now()
    });
    let parsed: CapabilityProof = serde_json::from_value(proof_json).unwrap();
    assert!(parsed.verified);
}

/// Test message construction for agent context.
#[test]
fn agent_context() {
    use servitor::agent::{ContentBlock, ConversationContext};

    let mut ctx = ConversationContext::new();

    ctx.add_user_message("Hello, execute a task");
    ctx.add_assistant_message(vec![
        ContentBlock::text("I'll help with that."),
        ContentBlock::tool_use(
            "call_1",
            "shell_execute",
            serde_json::json!({"command": "ls"}),
        ),
    ]);
    ctx.add_tool_results(vec![ContentBlock::tool_result(
        "call_1",
        "file1.txt\nfile2.txt",
        false,
    )]);

    assert_eq!(ctx.messages().len(), 3);
    assert_eq!(ctx.turn_count(), 1);
}
