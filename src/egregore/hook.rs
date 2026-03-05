//! Hook receiver — parses stdin JSON from egregore hooks.
//!
//! When configured as an egregore hook, Servitor receives message JSON on stdin.
//! This module handles parsing and validation of incoming messages.

use std::io::{self, BufRead};

use crate::egregore::messages::EgregoreMessage;
use crate::error::{Result, ServitorError};

/// Read and parse a single message from stdin.
pub fn receive_message() -> Result<EgregoreMessage> {
    let stdin = io::stdin();
    let mut line = String::new();

    stdin.lock().read_line(&mut line).map_err(|e| ServitorError::Egregore {
        reason: format!("failed to read from stdin: {}", e),
    })?;

    if line.is_empty() {
        return Err(ServitorError::Egregore {
            reason: "empty input from stdin".into(),
        });
    }

    let message: EgregoreMessage = serde_json::from_str(&line)?;
    Ok(message)
}

/// Read all available messages from stdin (for batch processing).
pub fn receive_all_messages() -> Result<Vec<EgregoreMessage>> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let mut messages = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|e| ServitorError::Egregore {
            reason: format!("failed to read line: {}", e),
        })?;

        if line.is_empty() {
            continue;
        }

        let message: EgregoreMessage = serde_json::from_str(&line)?;
        messages.push(message);
    }

    Ok(messages)
}

/// Parse a message from a JSON string (for testing or direct invocation).
pub fn parse_message(json: &str) -> Result<EgregoreMessage> {
    let message: EgregoreMessage = serde_json::from_str(json)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_message_json() -> String {
        serde_json::json!({
            "author": "@test.ed25519",
            "sequence": 1,
            "timestamp": Utc::now(),
            "content": {
                "type": "task",
                "hash": "abc123",
                "prompt": "Count the items in the pantry"
            },
            "hash": "def456",
            "signature": "sig123",
            "tags": ["task"]
        }).to_string()
    }

    #[test]
    fn parse_task_message() {
        let json = sample_message_json();
        let msg = parse_message(&json).unwrap();

        assert_eq!(msg.content_type(), Some("task"));
        assert!(msg.as_task().is_some());
    }

    #[test]
    fn extract_task_from_message() {
        let json = sample_message_json();
        let msg = parse_message(&json).unwrap();
        let task = msg.as_task().unwrap();

        assert_eq!(task.prompt, "Count the items in the pantry");
    }
}
