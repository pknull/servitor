//! Startup recovery selection for the feed-backed task lifecycle.

use crate::egregore::EgregoreMessage;
use crate::identity::PublicId;
use std::collections::{HashMap, HashSet};

/// One unresolved lifecycle chain that was already offered by this Servitor.
#[derive(Debug, Clone)]
pub struct RecoveryCandidate {
    /// Original task envelope, replayed through the normal authorization path.
    pub task_message: EgregoreMessage,
    /// Latest assignment addressed to this Servitor, when one already exists.
    pub assignment_message: Option<EgregoreMessage>,
}

/// Select identity-bound, offered, unresolved task chains for startup replay.
pub fn select_recovery_candidates(
    messages: &[EgregoreMessage],
    servitor: &PublicId,
) -> Vec<RecoveryCandidate> {
    let mut tasks: HashMap<String, Vec<EgregoreMessage>> = HashMap::new();
    let mut offers: HashMap<String, EgregoreMessage> = HashMap::new();
    let mut withdrawals: HashMap<String, EgregoreMessage> = HashMap::new();
    let mut assignments: HashMap<String, EgregoreMessage> = HashMap::new();
    let mut resolved = HashSet::new();

    for message in messages {
        let Some(content) = message.content.as_ref() else {
            continue;
        };
        let Some(message_type) = content.get("type").and_then(|value| value.as_str()) else {
            continue;
        };

        match message_type {
            "task" => {
                let Some(task) = message.as_task() else {
                    continue;
                };
                tasks
                    .entry(task.effective_id().to_string())
                    .or_default()
                    .push(message.clone());
            }
            "task_offer" if is_identity_bound_servitor_record(message, content, servitor) => {
                if let Some(task_id) = task_id(content) {
                    insert_latest(&mut offers, task_id.to_string(), message.clone());
                }
            }
            "task_offer_withdraw"
                if is_identity_bound_servitor_record(message, content, servitor) =>
            {
                if let Some(task_id) = task_id(content) {
                    insert_latest(&mut withdrawals, task_id.to_string(), message.clone());
                }
            }
            "task_assign" if record_targets_servitor(content, servitor) => {
                let assigner_matches_author = content
                    .get("assigner")
                    .and_then(|value| value.as_str())
                    .map(|assigner| assigner == message.author.0)
                    .unwrap_or(true);
                if assigner_matches_author {
                    if let Some(task_id) = task_id(content) {
                        insert_latest(&mut assignments, task_id.to_string(), message.clone());
                    }
                }
            }
            "task_result" | "task_failed"
                if is_identity_bound_servitor_record(message, content, servitor) =>
            {
                if let Some(task_id) = task_id(content) {
                    resolved.insert(task_id.to_string());
                }
            }
            _ => {}
        }
    }

    let mut candidates = offers
        .into_iter()
        .filter_map(|(task_id, offer)| {
            if resolved.contains(&task_id) {
                return None;
            }
            if withdrawals
                .get(&task_id)
                .is_some_and(|withdrawal| is_later(withdrawal, &offer))
            {
                return None;
            }

            // An offer can only bind to a task already visible when the offer
            // was authored. Never recover a later task that reuses the same
            // caller-chosen lifecycle id.
            let task_message = tasks
                .remove(&task_id)?
                .into_iter()
                .filter(|task| !is_later(task, &offer))
                .max_by(compare_messages)?;

            Some(RecoveryCandidate {
                task_message,
                assignment_message: assignments.remove(&task_id),
            })
        })
        .collect::<Vec<_>>();

    // Feed reads are normally newest-first. Startup replay is made stable and
    // causal by restoring the oldest task chain first.
    candidates.sort_by(|left, right| {
        left.task_message
            .timestamp
            .cmp(&right.task_message.timestamp)
            .then_with(|| left.task_message.hash.cmp(&right.task_message.hash))
    });
    candidates
}

fn task_id(content: &serde_json::Value) -> Option<&str> {
    content.get("task_id").and_then(|value| value.as_str())
}

fn record_targets_servitor(content: &serde_json::Value, servitor: &PublicId) -> bool {
    content.get("servitor").and_then(|value| value.as_str()) == Some(servitor.0.as_str())
}

fn is_identity_bound_servitor_record(
    message: &EgregoreMessage,
    content: &serde_json::Value,
    servitor: &PublicId,
) -> bool {
    message.author == *servitor && record_targets_servitor(content, servitor)
}

fn insert_latest(
    records: &mut HashMap<String, EgregoreMessage>,
    task_id: String,
    message: EgregoreMessage,
) {
    match records.get(&task_id) {
        Some(existing) if !is_later(&message, existing) => {}
        _ => {
            records.insert(task_id, message);
        }
    }
}

fn is_later(left: &EgregoreMessage, right: &EgregoreMessage) -> bool {
    compare_messages(left, right).is_gt()
}

fn compare_messages(left: &EgregoreMessage, right: &EgregoreMessage) -> std::cmp::Ordering {
    (left.timestamp, left.sequence, &left.hash).cmp(&(right.timestamp, right.sequence, &right.hash))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use super::*;

    fn public_id(fill: char) -> PublicId {
        PublicId(format!("@{}.ed25519", fill.to_string().repeat(44)))
    }

    fn message(
        author: &PublicId,
        sequence: u64,
        timestamp_offset: i64,
        content: serde_json::Value,
    ) -> EgregoreMessage {
        EgregoreMessage {
            author: author.clone(),
            sequence,
            timestamp: Utc::now() + Duration::seconds(timestamp_offset),
            content: Some(content),
            hash: format!("message-{sequence}"),
            signature: "test-signature".to_string(),
            tags: vec![],
            relates: None,
            trace_id: None,
            span_id: None,
        }
    }

    fn task(author: &PublicId, sequence: u64, task_id: &str) -> EgregoreMessage {
        message(
            author,
            sequence,
            sequence as i64,
            json!({
                "type": "task",
                "id": task_id,
                "hash": task_id,
                "prompt": "recover me",
                "requestor": author,
                "tool_calls": [{
                    "id": "call-1",
                    "name": "test_echo",
                    "arguments": {"message": "hello"}
                }]
            }),
        )
    }

    fn offer(
        author: &PublicId,
        sequence: u64,
        task_id: &str,
        claimed_servitor: &PublicId,
    ) -> EgregoreMessage {
        message(
            author,
            sequence,
            sequence as i64,
            json!({
                "type": "task_offer",
                "task_id": task_id,
                "servitor": claimed_servitor,
                "capabilities": ["test"],
                "ttl_seconds": 300,
                "timestamp": Utc::now(),
            }),
        )
    }

    fn assignment(
        author: &PublicId,
        sequence: u64,
        task_id: &str,
        servitor: &PublicId,
    ) -> EgregoreMessage {
        message(
            author,
            sequence,
            sequence as i64,
            json!({
                "type": "task_assign",
                "task_id": task_id,
                "servitor": servitor,
                "assigner": author,
            }),
        )
    }

    fn result(servitor: &PublicId, sequence: u64, task_id: &str) -> EgregoreMessage {
        message(
            servitor,
            sequence,
            sequence as i64,
            json!({
                "type": "task_result",
                "task_id": task_id,
                "servitor": servitor,
                "correlation_id": "corr-1",
                "task_hash": task_id,
                "result_hash": "result-1",
                "status": "success"
            }),
        )
    }

    #[test]
    fn selects_offered_pending_task_for_coordinator_restore() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let messages = vec![
            task(&requestor, 1, "pending-task"),
            offer(&servitor, 2, "pending-task", &servitor),
        ];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert_eq!(selected.len(), 1);
        assert!(selected[0].assignment_message.is_none());
    }

    #[test]
    fn selects_one_candidate_for_duplicate_assignments() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let messages = vec![
            task(&requestor, 1, "assigned-task"),
            offer(&servitor, 2, "assigned-task", &servitor),
            assignment(&requestor, 3, "assigned-task", &servitor),
            assignment(&requestor, 4, "assigned-task", &servitor),
        ];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].assignment_message.as_ref().unwrap().sequence, 4);
    }

    #[test]
    fn excludes_author_mismatched_offer_and_authoritative_result() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let attacker = public_id('C');
        let messages = vec![
            task(&requestor, 1, "spoofed-offer"),
            offer(&attacker, 2, "spoofed-offer", &servitor),
            task(&requestor, 3, "completed-task"),
            offer(&servitor, 4, "completed-task", &servitor),
            assignment(&requestor, 5, "completed-task", &servitor),
            result(&servitor, 6, "completed-task"),
        ];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert!(selected.is_empty());
    }

    #[test]
    fn offer_recovery_does_not_substitute_later_task_with_reused_id() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let messages = vec![
            task(&requestor, 1, "reused-id"),
            offer(&servitor, 2, "reused-id", &servitor),
            task(&requestor, 3, "reused-id"),
            assignment(&requestor, 4, "reused-id", &servitor),
        ];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].task_message.sequence, 1);
    }
}
