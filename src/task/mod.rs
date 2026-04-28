//! Task coordination helpers for offer -> assign -> execute flow.

mod filter;
mod handlers;
mod state;

pub use filter::task_matches_capabilities;
pub use handlers::{execute_assigned_task, maybe_accept_assignment, process_sse_message};
pub use state::{
    ActiveExecution, AssignmentDecision, ExpiredOffer, OfferDecision, OfferedTask, TaskCoordinator,
    TaskLifecycleEvent,
};

use crate::authority::{AuthRequest, AuthResult, Authority, PersonId};
use crate::egregore::{
    EgregoreClient, EgregoreMessage, Task, TaskFailed, TaskFailureReason,
};
use crate::error::Result;
use crate::identity::{Identity, PublicId};

/// Canonical rejection message published when a task arrives without
/// pre-planned `tool_calls`. Locked across every rejection site so wording
/// can't drift between hook / SSE / daemon execution paths.
pub(crate) const MISSING_TOOL_CALLS_REJECTION_REASON: &str =
    "Servitor requires pre-planned tool_calls. Route through familiar for task decomposition.";

/// Publish a `task_failed` rejecting an inbound task that arrived without
/// `tool_calls`. The published payload carries the canonical rejection
/// reason and the inherited task trace; per-site cleanup (finish_execution,
/// runtime stats, return shape) stays at the caller.
///
/// Returns `Result<()>` (publish hash is discarded) so callers can `?` it
/// (hook, SSE) or `let _ =` it (daemon). Does NOT log — pre-publish tracing
/// stays at the call site so the operator log retains its mode-specific
/// context.
pub(crate) async fn publish_missing_tool_calls_rejection(
    egregore: &EgregoreClient,
    identity: &Identity,
    task: &Task,
) -> Result<()> {
    let task_trace_id = task.context_trace_id();
    let failed = TaskFailed::new(
        task.effective_id().to_string(),
        identity.public_id(),
        TaskFailureReason::ExecutionError,
        Some(MISSING_TOOL_CALLS_REJECTION_REASON.into()),
    );
    egregore
        .publish_failed(&failed, task_trace_id.as_deref(), None)
        .await?;
    Ok(())
}

/// Build the request authorization skill string for a task.
pub fn request_skill(task: &Task) -> String {
    format!("request:{}", task.effective_task_type())
}

/// Build the assignment authorization skill string for a task.
pub fn assign_skill(task: &Task) -> String {
    format!("assign:{}", task.effective_task_type())
}

/// Copy top-level message trace identifiers into task context when absent.
pub fn inherit_trace_context(task: &mut Task, message: &EgregoreMessage) {
    if !task.context.contains_key("trace_id") {
        if let Some(trace_id) = &message.trace_id {
            task.context.insert(
                "trace_id".to_string(),
                serde_json::Value::String(trace_id.clone()),
            );
        }
    }

    if !task.context.contains_key("span_id") {
        if let Some(span_id) = &message.span_id {
            task.context.insert(
                "span_id".to_string(),
                serde_json::Value::String(span_id.clone()),
            );
        }
    }
}

/// Check whether a requestor is allowed to ask for this task type.
pub fn authorize_offer_request(
    authority: &Authority,
    requestor: &PublicId,
    task: &Task,
) -> AuthResult {
    authority.authorize(&AuthRequest {
        person: PersonId::from_egregore(&requestor.0),
        skill: request_skill(task),
    })
}

/// Check whether an assigner may assign work for this task.
pub fn authorize_assignment(
    authority: &Authority,
    assigner: &PublicId,
    requestor: &PublicId,
    task: &Task,
) -> bool {
    if assigner == requestor {
        return true;
    }

    authority
        .authorize(&AuthRequest {
            person: PersonId::from_egregore(&assigner.0),
            skill: assign_skill(task),
        })
        .allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::AuthorityConfig;
    use chrono::Utc;

    fn test_authority() -> Authority {
        Authority::from_config(
            AuthorityConfig::from_toml(
                r#"
[[keeper]]
name = "requestor"
egregore = "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519"

[[keeper]]
name = "assigner"
egregore = "@BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=.ed25519"

[[permission]]
keeper = "requestor"
skills = ["request:docker:*"]

[[permission]]
keeper = "assigner"
skills = ["assign:*"]
"#,
            )
            .unwrap(),
        )
    }

    fn test_task() -> Task {
        Task {
            msg_type: "task".to_string(),
            id: Some("task-1".to_string()),
            hash: "hash-1".to_string(),
            task_type: Some("docker:deploy:staging".to_string()),
            request: Some("deploy staging".to_string()),
            requestor: Some(PublicId(
                "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string(),
            )),
            prompt: "deploy staging".to_string(),
            required_caps: vec!["docker".to_string()],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: Some(60),
            author: None,
            keeper: None,
            scope_override: None,
            tool_calls: vec![],
            depends_on: vec![],
        }
    }

    #[test]
    fn offer_gate_uses_request_permission() {
        let auth = test_authority();
        let task = test_task();
        let result = authorize_offer_request(&auth, task.requestor.as_ref().unwrap(), &task);
        assert!(result.allowed);
    }

    #[test]
    fn assignment_allows_original_requestor() {
        let auth = test_authority();
        let task = test_task();
        let requestor = task.requestor.as_ref().unwrap();
        assert!(authorize_assignment(&auth, requestor, requestor, &task));
    }

    #[test]
    fn assignment_requires_assign_permission_for_others() {
        let auth = test_authority();
        let task = test_task();
        let requestor = task.requestor.as_ref().unwrap();
        let assigner =
            PublicId("@BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=.ed25519".to_string());
        assert!(authorize_assignment(&auth, &assigner, requestor, &task));

        let stranger =
            PublicId("@CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=.ed25519".to_string());
        assert!(!authorize_assignment(&auth, &stranger, requestor, &task));
    }

    #[test]
    fn inherit_trace_context_copies_message_trace_fields() {
        let mut task = test_task();
        let message = EgregoreMessage {
            author: PublicId("@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string()),
            sequence: 1,
            timestamp: Utc::now(),
            content: None,
            hash: "hash-1".to_string(),
            signature: "sig".to_string(),
            tags: vec![],
            relates: None,
            trace_id: Some("trace-123".to_string()),
            span_id: Some("span-456".to_string()),
        };

        inherit_trace_context(&mut task, &message);

        assert_eq!(
            task.context.get("trace_id").and_then(|v| v.as_str()),
            Some("trace-123")
        );
        assert_eq!(
            task.context.get("span_id").and_then(|v| v.as_str()),
            Some("span-456")
        );
    }
}
