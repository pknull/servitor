//! Task construction from comms messages.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::CommsMessage;
use crate::egregore::Task;

/// Build a Task from a CommsMessage.
///
/// Generates a content-based hash from user_id, content, and timestamp
/// to uniquely identify the task.
pub fn task_from_comms(msg: &CommsMessage) -> Task {
    let mut hasher = Sha256::new();
    hasher.update(msg.user_id.as_bytes());
    hasher.update(msg.content.as_bytes());
    hasher.update(msg.timestamp.timestamp().to_le_bytes());
    let hash = hasher.finalize();
    let hash_str: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    let mut context = HashMap::new();
    context.insert("source".to_string(), serde_json::json!(msg.source.name()));
    context.insert(
        "user".to_string(),
        serde_json::json!({
            "id": msg.user_id,
            "name": msg.user_name,
        }),
    );
    context.insert("channel".to_string(), serde_json::json!(msg.channel_id));

    Task {
        msg_type: "task".to_string(),
        id: None,
        hash: hash_str,
        task_type: None,
        request: Some(msg.content.clone()),
        requestor: None,
        prompt: msg.content.clone(),
        required_caps: vec![],
        parent_id: msg.reply_to.clone(),
        context,
        scope_override: None,
        priority: 0,
        timeout_secs: None,
        author: None,
        keeper: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::CommsSource;
    use chrono::Utc;

    #[test]
    fn task_from_comms_builds_valid_task() {
        let msg = CommsMessage {
            source: CommsSource::Discord {
                guild_id: "guild-1".to_string(),
                guild_name: "Test Guild".to_string(),
            },
            channel_id: "channel-1".to_string(),
            user_id: "user-123".to_string(),
            user_name: "TestUser".to_string(),
            content: "deploy staging".to_string(),
            reply_to: None,
            message_id: "msg-456".to_string(),
            timestamp: Utc::now(),
        };

        let task = task_from_comms(&msg);

        assert_eq!(task.msg_type, "task");
        assert_eq!(task.prompt, "deploy staging");
        assert_eq!(task.request, Some("deploy staging".to_string()));
        assert!(!task.hash.is_empty());
        assert_eq!(task.hash.len(), 64); // SHA256 hex
    }

    #[test]
    fn task_hash_is_deterministic() {
        let timestamp = Utc::now();
        let msg = CommsMessage {
            source: CommsSource::Discord {
                guild_id: "guild-1".to_string(),
                guild_name: "Test".to_string(),
            },
            channel_id: "ch".to_string(),
            user_id: "user".to_string(),
            user_name: "User".to_string(),
            content: "test".to_string(),
            reply_to: None,
            message_id: "msg".to_string(),
            timestamp,
        };

        let task1 = task_from_comms(&msg);
        let task2 = task_from_comms(&msg);

        assert_eq!(task1.hash, task2.hash);
    }
}
