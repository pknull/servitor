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
    pub assignment: Option<RecoveryAssignment>,
}

#[derive(Debug, Clone)]
pub enum RecoveryAssignment {
    Bare(EgregoreMessage),
    AcceptedTyped(EgregoreMessage),
}

#[derive(Debug, Clone)]
struct AcceptedTypedAssignment {
    command: EgregoreMessage,
    result: EgregoreMessage,
}

/// Select identity-bound, offered, unresolved task chains for startup replay.
pub fn select_recovery_candidates(
    messages: &[EgregoreMessage],
    servitor: &PublicId,
) -> Vec<RecoveryCandidate> {
    let mut tasks: HashMap<String, Vec<EgregoreMessage>> = HashMap::new();
    let mut offers: HashMap<String, Vec<EgregoreMessage>> = HashMap::new();
    let mut withdrawals: HashMap<String, EgregoreMessage> = HashMap::new();
    let mut assignments: HashMap<String, EgregoreMessage> = HashMap::new();
    let mut typed_commands: HashMap<String, Vec<EgregoreMessage>> = HashMap::new();
    let mut typed_acceptances: HashMap<String, EgregoreMessage> = HashMap::new();
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
                    offers
                        .entry(task_id.to_string())
                        .or_default()
                        .push(message.clone());
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
            "assign_task" if record_targets_executor(content, servitor) => {
                let assigner_matches_author =
                    content.get("assigner").and_then(|value| value.as_str())
                        == Some(message.author.0.as_str());
                if assigner_matches_author {
                    if let Some(command_id) = command_id(content) {
                        typed_commands
                            .entry(command_id.to_string())
                            .or_default()
                            .push(message.clone());
                    }
                }
            }
            "assign_task_result" if message.author == *servitor => {
                let accepted =
                    content.get("status").and_then(|value| value.as_str()) == Some("accepted");
                if accepted {
                    if let Some(command_id) = command_id(content) {
                        insert_latest(
                            &mut typed_acceptances,
                            command_id.to_string(),
                            message.clone(),
                        );
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

    let mut accepted_typed_by_task = HashMap::new();
    for (command_id, result) in typed_acceptances {
        let Some(result_content) = result.content.as_ref() else {
            continue;
        };
        let Some(accepted_command_hash) = result_content
            .get("lifecycle_hash")
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let Some(command) = typed_commands.remove(&command_id).and_then(|commands| {
            commands
                .into_iter()
                .find(|command| command.hash == accepted_command_hash)
        }) else {
            continue;
        };
        let Some(command_content) = command.content.as_ref() else {
            continue;
        };
        let Some(command_task_id) = task_id(command_content) else {
            continue;
        };
        if task_id(result_content) != Some(command_task_id) {
            continue;
        }

        insert_latest_accepted_typed(
            &mut accepted_typed_by_task,
            command_task_id.to_string(),
            AcceptedTypedAssignment { command, result },
        );
    }

    let mut candidates = offers
        .into_iter()
        .filter_map(|(task_id, task_offers)| {
            if resolved.contains(&task_id) {
                return None;
            }
            let task_messages = tasks.remove(&task_id)?;

            // An accepted command is the durable assignment marker. Match its
            // exact task and offer hashes even when a crash/retry has appended
            // newer delivery attempts or recovery offers to the same chain.
            if let Some(accepted) = accepted_typed_by_task.remove(&task_id) {
                let content = accepted.command.content.as_ref()?;
                let accepted_offer_hash = content.get("offer_hash")?.as_str()?;
                let accepted_task_hash = content.get("task_hash")?.as_str()?;
                let offer = task_offers
                    .iter()
                    .find(|offer| offer.hash == accepted_offer_hash)?;
                let task_message = task_messages
                    .iter()
                    .find(|task| task.hash == accepted_task_hash)?;
                let predecessor_matches = content
                    .get("predecessor")
                    .and_then(|value| value.as_str())
                    .is_none_or(|predecessor| predecessor == offer.hash);

                if predecessor_matches {
                    return Some(RecoveryCandidate {
                        task_message: task_message.clone(),
                        assignment: Some(RecoveryAssignment::AcceptedTyped(accepted.command)),
                    });
                }
            }

            let offer = task_offers.into_iter().max_by(compare_messages)?;
            if withdrawals
                .get(&task_id)
                .is_some_and(|withdrawal| is_later(withdrawal, &offer))
            {
                return None;
            }

            // An offer can only bind to a task already visible when the offer
            // was authored. Never recover a later task that reuses the same
            // caller-chosen lifecycle id.
            let task_message = task_messages
                .into_iter()
                .filter(|task| !is_later(task, &offer))
                .max_by(compare_messages)?;
            let assignment = assignments.remove(&task_id).map(RecoveryAssignment::Bare);

            Some(RecoveryCandidate {
                task_message,
                assignment,
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

fn record_targets_executor(content: &serde_json::Value, servitor: &PublicId) -> bool {
    content.get("executor").and_then(|value| value.as_str()) == Some(servitor.0.as_str())
}

fn command_id(content: &serde_json::Value) -> Option<&str> {
    content.get("command_id").and_then(|value| value.as_str())
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

fn insert_latest_accepted_typed(
    records: &mut HashMap<String, AcceptedTypedAssignment>,
    task_id: String,
    accepted: AcceptedTypedAssignment,
) {
    match records.get(&task_id) {
        Some(existing) if !is_later(&accepted.result, &existing.result) => {}
        _ => {
            records.insert(task_id, accepted);
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

    fn typed_assignment(
        author: &PublicId,
        sequence: u64,
        task_id: &str,
        task_hash: &str,
        offer_hash: &str,
        servitor: &PublicId,
        command_id: &str,
    ) -> EgregoreMessage {
        message(
            author,
            sequence,
            sequence as i64,
            json!({
                "type": "assign_task",
                "task_id": task_id,
                "task_hash": task_hash,
                "offer_hash": offer_hash,
                "executor": servitor,
                "assigner": author,
                "predecessor": offer_hash,
                "command_id": command_id,
                "attempt_id": "attempt-1",
                "expires_at": Utc::now() + Duration::minutes(5),
            }),
        )
    }

    fn typed_acceptance(
        servitor: &PublicId,
        sequence: u64,
        task_id: &str,
        command_id: &str,
        command_hash: &str,
    ) -> EgregoreMessage {
        message(
            servitor,
            sequence,
            sequence as i64,
            json!({
                "type": "assign_task_result",
                "command_id": command_id,
                "status": "accepted",
                "task_id": task_id,
                "lifecycle_hash": command_hash,
                "task_state": "assigned",
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
        assert!(selected[0].assignment.is_none());
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
        match selected[0].assignment.as_ref().unwrap() {
            RecoveryAssignment::Bare(message) => assert_eq!(message.sequence, 4),
            RecoveryAssignment::AcceptedTyped(_) => panic!("expected bare assignment"),
        }
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

    #[test]
    fn selects_accepted_typed_assignment_without_execution_result() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let task_message = task(&requestor, 1, "typed-recovery");
        let offer_message = offer(&servitor, 2, "typed-recovery", &servitor);
        let command_message = typed_assignment(
            &requestor,
            3,
            "typed-recovery",
            &task_message.hash,
            &offer_message.hash,
            &servitor,
            "command-1",
        );
        let acceptance = typed_acceptance(
            &servitor,
            4,
            "typed-recovery",
            "command-1",
            &command_message.hash,
        );
        let messages = vec![task_message, offer_message, command_message, acceptance];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert_eq!(selected.len(), 1);
        assert!(matches!(
            selected[0].assignment,
            Some(RecoveryAssignment::AcceptedTyped(_))
        ));
    }

    #[test]
    fn accepted_typed_recovery_survives_newer_offer_and_delivery_attempt() {
        let requestor = public_id('A');
        let servitor = public_id('B');
        let task_message = task(&requestor, 1, "typed-retry-recovery");
        let accepted_offer = offer(&servitor, 2, "typed-retry-recovery", &servitor);
        let accepted_command = typed_assignment(
            &requestor,
            3,
            "typed-retry-recovery",
            &task_message.hash,
            &accepted_offer.hash,
            &servitor,
            "command-1",
        );
        let acceptance = typed_acceptance(
            &servitor,
            4,
            "typed-retry-recovery",
            "command-1",
            &accepted_command.hash,
        );
        let replay_offer = offer(&servitor, 5, "typed-retry-recovery", &servitor);
        let retry = typed_assignment(
            &requestor,
            6,
            "typed-retry-recovery",
            &task_message.hash,
            &accepted_offer.hash,
            &servitor,
            "command-1",
        );
        let messages = vec![
            task_message,
            accepted_offer,
            accepted_command,
            acceptance,
            replay_offer,
            retry,
        ];

        let selected = select_recovery_candidates(&messages, &servitor);

        assert_eq!(selected.len(), 1);
        match selected[0].assignment.as_ref().unwrap() {
            RecoveryAssignment::AcceptedTyped(message) => assert_eq!(message.sequence, 3),
            RecoveryAssignment::Bare(_) => panic!("expected accepted typed assignment"),
        }
    }
}
