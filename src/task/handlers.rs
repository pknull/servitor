//! Task message handlers for the daemon event loop.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::authority::{Authority, DenialReason, PersonId};
use crate::config::Config;
use crate::egregore::{
    AssignTaskResultStatus, AssignTaskResultV1, AssignTaskV1, AuthGate, EgregoreClient,
    EgregoreMessage, FailureDetail, TaskAssign, TaskFailed, TaskPing, TaskStatusMessage,
};
use crate::error::{Result, ServitorError};
use crate::events::sse::SseSource;
use crate::identity::Identity;
use crate::runtime::{publish_auth_denied_event, RuntimeContext};
use crate::task::{
    assign_skill, authorize_assignment, authorize_offer_request, inherit_trace_context,
    request_skill, signed_requestor, task_matches_capabilities, AssignmentDecision,
    TaskCoordinator, TaskLifecycleEvent,
};

const ASSIGNMENT_FEED_QUERY_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy)]
struct AssignTaskValidation<'a> {
    duplicate: bool,
    resolved: bool,
    authorized: bool,
    task_hash: Option<&'a str>,
    offer_hash: Option<&'a str>,
    latest_lifecycle_hash: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignTaskDecision {
    Ignore,
    Duplicate,
    Reject(&'static str),
    Accept,
}

fn decide_assign_task(
    command: &AssignTaskV1,
    message_author: &crate::identity::PublicId,
    servitor: &crate::identity::PublicId,
    now: DateTime<Utc>,
    validation: AssignTaskValidation<'_>,
) -> AssignTaskDecision {
    if command.executor != *servitor {
        return AssignTaskDecision::Ignore;
    }
    if validation.duplicate {
        return AssignTaskDecision::Duplicate;
    }
    if command.assigner != *message_author {
        return AssignTaskDecision::Reject("assigner_mismatch");
    }
    if command.expires_at < now {
        return AssignTaskDecision::Reject("expired");
    }
    if validation.resolved {
        return AssignTaskDecision::Reject("task_resolved");
    }
    let Some(task_hash) = validation.task_hash else {
        return AssignTaskDecision::Reject("offer_unavailable");
    };
    let Some(offer_hash) = validation.offer_hash else {
        return AssignTaskDecision::Reject("offer_unavailable");
    };
    if command.task_hash != task_hash {
        return AssignTaskDecision::Reject("task_mismatch");
    }
    if command.offer_hash != offer_hash {
        return AssignTaskDecision::Reject("offer_unavailable");
    }
    if command
        .predecessor
        .as_deref()
        .is_some_and(|predecessor| validation.latest_lifecycle_hash != Some(predecessor))
    {
        return AssignTaskDecision::Reject("stale_predecessor");
    }
    if !validation.authorized {
        return AssignTaskDecision::Reject("unauthorized");
    }
    AssignTaskDecision::Accept
}

fn is_recorded_command_result(
    messages: &[EgregoreMessage],
    servitor: &crate::identity::PublicId,
    command_id: &str,
) -> bool {
    messages.iter().any(|message| {
        message.author == *servitor
            && message
                .as_assign_task_result()
                .is_some_and(|result| result.command_id == command_id)
    })
}

fn task_has_accepted_assignment(
    messages: &[EgregoreMessage],
    servitor: &crate::identity::PublicId,
    task_id: &str,
) -> bool {
    messages.iter().any(|message| {
        message.author == *servitor
            && message.as_assign_task_result().is_some_and(|result| {
                result.task_id == task_id && result.status == AssignTaskResultStatus::Accepted
            })
    })
}

fn task_has_terminal_record(
    messages: &[EgregoreMessage],
    servitor: &crate::identity::PublicId,
    task_id: &str,
) -> bool {
    messages.iter().any(|message| {
        if message.author != *servitor {
            return false;
        }
        let Some(content) = message.content.as_ref() else {
            return false;
        };
        matches!(message.content_type(), Some("task_result" | "task_failed"))
            && content.get("task_id").and_then(|value| value.as_str()) == Some(task_id)
            && content.get("servitor").and_then(|value| value.as_str()) == Some(servitor.0.as_str())
    })
}

/// Process an SSE message and potentially return an assignment decision.
pub async fn process_sse_message(
    message: &EgregoreMessage,
    authority: &Authority,
    identity: &Identity,
    capability_set: &HashSet<String>,
    egregore: &EgregoreClient,
    task_coordinator: &mut TaskCoordinator,
    config: &Config,
) -> Result<Option<AssignmentDecision>> {
    if let Some(mut task) = message.as_task() {
        task.author = Some(message.author.0.clone());
        task.normalize(Some(&message.author));

        if !task_matches_capabilities(&task, capability_set) {
            return Ok(None);
        }

        let Some(requestor) = signed_requestor(&task, &message.author) else {
            return Ok(None);
        };

        let auth_result = authorize_offer_request(authority, &requestor, &task);

        if !auth_result.allowed {
            publish_auth_denied_event(
                egregore,
                identity,
                authority,
                &PersonId::from_egregore(requestor.0.clone()),
                &request_skill(&task),
                AuthGate::Offer,
                auth_result
                    .denial
                    .unwrap_or(DenialReason::NoMatchingPermission),
            )
            .await;
            tracing::info!(
                author = %requestor,
                task_type = %task.effective_task_type(),
                reason = %auth_result.reason,
                "skipping unauthorized task request"
            );
            return Ok(None);
        }

        task.keeper = auth_result.keeper;
        inherit_trace_context(&mut task, message);

        let task_trace_id = task.inherited_trace_id();
        let task_id = task.effective_id().to_string();
        let offer = task_coordinator
            .register_offer(
                task,
                requestor,
                capability_set.iter().cloned().collect(),
                message.hash.clone(),
            )
            .offer;
        let offer_hash = egregore
            .publish_offer(&offer, task_trace_id.as_deref(), None)
            .await?;
        if !task_coordinator.record_offer_hash(&task_id, offer_hash) {
            tracing::warn!(task_id, "published offer was no longer pending");
        }

        return Ok(None);
    }

    if let Some(assign) = message.as_assign_task() {
        return maybe_accept_typed_assignment(
            assign,
            message,
            egregore,
            authority,
            identity,
            task_coordinator,
            config,
        )
        .await;
    }

    if let Some(assign) = message.as_task_assign() {
        return maybe_accept_assignment(
            assign,
            message,
            egregore,
            authority,
            identity,
            task_coordinator,
            config,
        )
        .await;
    }

    Ok(None)
}

/// Validate and durably accept or reject a typed assignment command.
async fn maybe_accept_typed_assignment(
    assign: AssignTaskV1,
    message: &EgregoreMessage,
    egregore: &EgregoreClient,
    authority: &Authority,
    identity: &Identity,
    task_coordinator: &mut TaskCoordinator,
    config: &Config,
) -> Result<Option<AssignmentDecision>> {
    let servitor = identity.public_id();
    if assign.executor != servitor {
        return Ok(None);
    }

    // The durable result is the reply. Check it before all other validation
    // so a retry cannot publish another decision or execute a second time.
    let assignment_results = egregore
        .query_messages(Some("assign_task_result"), ASSIGNMENT_FEED_QUERY_LIMIT)
        .await?;
    if is_recorded_command_result(&assignment_results, &servitor, &assign.command_id) {
        tracing::debug!(
            command_id = %assign.command_id,
            task_id = %assign.task_id,
            "typed assignment already has an owner result"
        );
        return Ok(None);
    }
    // A full window cannot prove that an older result does not exist. Refuse
    // mutation rather than weakening command idempotency at the scan bound.
    if assignment_results.len() == ASSIGNMENT_FEED_QUERY_LIMIT {
        return Err(ServitorError::Egregore {
            reason: format!(
                "typed assignment result history exceeded bounded lookup window ({ASSIGNMENT_FEED_QUERY_LIMIT})"
            ),
        });
    }

    let task_results = egregore
        .query_messages(Some("task_result"), ASSIGNMENT_FEED_QUERY_LIMIT)
        .await?;
    let task_failures = egregore
        .query_messages(Some("task_failed"), ASSIGNMENT_FEED_QUERY_LIMIT)
        .await?;
    let pending = task_coordinator.pending_offer(&assign.task_id).cloned();
    let authorized = pending.as_ref().is_some_and(|offered| {
        authorize_assignment(
            authority,
            &assign.assigner,
            &offered.requestor,
            &offered.task,
        )
    });
    let resolved = task_coordinator.is_active(&assign.task_id)
        || task_has_accepted_assignment(&assignment_results, &servitor, &assign.task_id)
        || task_has_terminal_record(&task_results, &servitor, &assign.task_id)
        || task_has_terminal_record(&task_failures, &servitor, &assign.task_id);
    let offer_hash = pending
        .as_ref()
        .and_then(|offered| offered.offer_hash.as_deref());
    let validation = AssignTaskValidation {
        duplicate: false,
        resolved,
        authorized,
        task_hash: pending
            .as_ref()
            .map(|offered| offered.task_message_hash.as_str()),
        offer_hash,
        latest_lifecycle_hash: offer_hash,
    };

    match decide_assign_task(&assign, &message.author, &servitor, Utc::now(), validation) {
        AssignTaskDecision::Ignore | AssignTaskDecision::Duplicate => Ok(None),
        AssignTaskDecision::Reject(reason) => {
            if reason == "unauthorized" {
                if let Some(offered) = pending.as_ref() {
                    publish_auth_denied_event(
                        egregore,
                        identity,
                        authority,
                        &PersonId::from_egregore(assign.assigner.0.clone()),
                        &assign_skill(&offered.task),
                        AuthGate::Assignment,
                        DenialReason::AssignmentDenied,
                    )
                    .await;
                }
            }
            let result = AssignTaskResultV1::rejected(&assign, reason);
            egregore.publish_assign_task_result(&result).await?;
            Ok(None)
        }
        AssignTaskDecision::Accept => {
            let result = AssignTaskResultV1::accepted(&assign, message.hash.clone());
            egregore.publish_assign_task_result(&result).await?;

            let legacy_assignment = TaskAssign {
                msg_type: "task_assign".to_string(),
                task_id: assign.task_id,
                servitor: assign.executor,
                assigner: Some(assign.assigner),
            };
            maybe_accept_assignment(
                legacy_assignment,
                message,
                egregore,
                authority,
                identity,
                task_coordinator,
                config,
            )
            .await
        }
    }
}

/// Check and potentially accept a task assignment.
pub async fn maybe_accept_assignment(
    assign: TaskAssign,
    message: &EgregoreMessage,
    egregore: &EgregoreClient,
    authority: &Authority,
    identity: &Identity,
    task_coordinator: &mut TaskCoordinator,
    config: &Config,
) -> Result<Option<AssignmentDecision>> {
    if assign.servitor != identity.public_id() {
        return Ok(None);
    }

    if let Some(content_assigner) = &assign.assigner {
        if content_assigner != &message.author {
            tracing::warn!(
                task_id = %assign.task_id,
                author = %message.author,
                assigner = %content_assigner,
                "ignoring task_assign with mismatched assigner"
            );
            return Ok(None);
        }
    }

    let Some(requestor) = task_coordinator.pending_requestor(&assign.task_id).cloned() else {
        return Ok(None);
    };
    let Some(task) = task_coordinator.pending_task(&assign.task_id).cloned() else {
        return Ok(None);
    };
    let assigner = assign
        .assigner
        .clone()
        .unwrap_or_else(|| message.author.clone());

    if !authorize_assignment(authority, &assigner, &requestor, &task) {
        publish_auth_denied_event(
            egregore,
            identity,
            authority,
            &PersonId::from_egregore(assigner.0.clone()),
            &assign_skill(&task),
            AuthGate::Assignment,
            DenialReason::AssignmentDenied,
        )
        .await;
        tracing::info!(
            task_id = %assign.task_id,
            assigner = %assigner,
            "ignoring unauthorized task assignment"
        );
        return Ok(None);
    }

    let eta_seconds = task.timeout_secs.unwrap_or(config.agent.timeout_secs);
    let decision = task_coordinator.apply_assignment(&assign, Instant::now(), eta_seconds);
    if let Some(decision) = decision {
        if task_coordinator.active_execution_count() > 1 {
            let _ = task_coordinator.finish_execution(&assign.task_id);
            task_coordinator.enqueue_assignment(decision);
            return Ok(None);
        }

        if task_coordinator.has_active_execution() {
            return Ok(Some(decision));
        }
    }

    Ok(None)
}

/// Execute an assigned task with timeout and SSE message handling.
///
/// Uses direct execution only — tasks must have pre-planned tool_calls.
pub(crate) async fn execute_assigned_task(
    assigned: AssignmentDecision,
    ctx: &RuntimeContext,
    config: &Config,
    mut sse_source: Option<&mut SseSource>,
    task_coordinator: &mut TaskCoordinator,
    capability_set: &HashSet<String>,
) -> Result<()> {
    let task_id = assigned.task.effective_id().to_string();
    let servitor_id = ctx.identity.public_id();
    let eta_seconds = assigned.started.eta_seconds;
    let task_trace_id = assigned.task.inherited_trace_id();

    // Reject tasks without pre-planned tool calls
    if !assigned.task.is_direct() {
        crate::task::publish_missing_tool_calls_rejection(
            &ctx.egregore,
            &ctx.identity,
            &assigned.task,
        )
        .await?;
        let _ = task_coordinator.finish_execution(&task_id);
        return Ok(());
    }

    ctx.egregore
        .publish_started(&assigned.started, task_trace_id.as_deref(), None)
        .await?;

    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(eta_seconds);

    let execution = crate::agent::direct::execute_direct(
        &assigned.task,
        &ctx.mcp_pool,
        &ctx.scope_enforcer,
        &ctx.identity,
        &config.agent,
        Some(&ctx.egregore),
        Some(&ctx.authority),
        assigned.task.keeper.as_deref(),
    );
    tokio::pin!(execution);

    loop {
        tokio::select! {
            result = &mut execution => {
                match result {
                    Ok(mut task_result) => {
                        task_result.task_id = task_id.clone();
                        task_result.servitor = servitor_id.clone();
                        task_result.duration_seconds = task_coordinator
                            .finish_execution(&task_id)
                            .map(|active| active.started_at.elapsed().as_secs());
                        ctx.egregore.publish_result(&task_result).await?;
                    }
                    Err(error) => {
                        let _ = task_coordinator.finish_execution(&task_id);
                        // The error text stays in the local log; the feed gets
                        // a bounded reason code and public context only.
                        tracing::warn!(task_id = %task_id, error = %error, "assigned task failed");
                        let failed = TaskFailed::new(
                            task_id.clone(),
                            servitor_id.clone(),
                            FailureDetail::from_execution_error(&error),
                        );
                        ctx.egregore
                            .publish_failed(&failed, task_trace_id.as_deref(), None)
                            .await?;
                    }
                }
                return Ok(());
            }
            _ = interval.tick() => {
                if Instant::now() >= deadline {
                    let _ = task_coordinator.finish_execution(&task_id);
                    let failed = TaskFailed::new(
                        task_id.clone(),
                        servitor_id.clone(),
                        FailureDetail::ExecutionTimeout { seconds: eta_seconds },
                    );
                    ctx.egregore
                        .publish_failed(&failed, task_trace_id.as_deref(), None)
                        .await?;
                    return Ok(());
                }

                if let Some(source) = sse_source.as_deref_mut() {
                    if let Some(message) = source.next_message().await {
                        if let Some(TaskPing { task_id: ping_task_id, .. }) = message.as_task_ping() {
                            if ping_task_id == task_id {
                                let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
                                let status = TaskStatusMessage::new(
                                    task_id.clone(),
                                    servitor_id.clone(),
                                    Some(remaining),
                                    Some("Task is still running.".to_string()),
                                );
                                ctx.egregore
                                    .publish_status(&status, task_trace_id.as_deref(), None)
                                    .await?;
                                continue;
                            }
                        }

                        if message.as_task().is_some()
                            || message.as_task_assign().is_some()
                            || message.as_assign_task().is_some()
                        {
                            if let Some(assigned) = process_sse_message(
                                &message,
                                &ctx.authority,
                                &ctx.identity,
                                capability_set,
                                &ctx.egregore,
                                task_coordinator,
                                config,
                            )
                            .await? {
                                tracing::info!(
                                    current_task = %task_id,
                                    queued_task = %assigned.task.effective_id(),
                                    "queued assignment while another task is running"
                                );
                                task_coordinator.enqueue_assignment(assigned);
                            }
                        }
                    }
                }

                for event in task_coordinator.collect_timeouts(Instant::now()) {
                    match event {
                        TaskLifecycleEvent::Withdraw(withdraw) => {
                            let _ = ctx.egregore.publish_offer_withdraw(&withdraw).await;
                        }
                        TaskLifecycleEvent::Failed(failed) => {
                            let _ = ctx.egregore.publish_failed(&failed, None, None).await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, Utc};

    use super::*;
    use crate::egregore::AssignTaskV1;

    fn public_id(fill: char) -> crate::identity::PublicId {
        crate::identity::PublicId(format!("@{}.ed25519", fill.to_string().repeat(44)))
    }

    fn command(
        executor: &crate::identity::PublicId,
        assigner: &crate::identity::PublicId,
    ) -> AssignTaskV1 {
        AssignTaskV1 {
            msg_type: "assign_task".to_string(),
            task_id: "task-1".to_string(),
            task_hash: "a".repeat(64),
            offer_hash: "b".repeat(64),
            executor: executor.clone(),
            assigner: assigner.clone(),
            predecessor: Some("b".repeat(64)),
            command_id: "command-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            expires_at: Utc::now() + ChronoDuration::minutes(5),
        }
    }

    fn valid_context() -> AssignTaskValidation<'static> {
        AssignTaskValidation {
            duplicate: false,
            resolved: false,
            authorized: true,
            task_hash: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            offer_hash: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            latest_lifecycle_hash: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        }
    }

    #[test]
    fn typed_assignment_validation_matrix_defaults_to_deny() {
        let executor = public_id('B');
        let assigner = public_id('A');
        let now = Utc::now();
        let valid = command(&executor, &assigner);

        assert_eq!(
            decide_assign_task(&valid, &assigner, &executor, now, valid_context()),
            AssignTaskDecision::Accept
        );

        let mut expired = valid.clone();
        expired.expires_at = now - ChronoDuration::seconds(1);
        assert_eq!(
            decide_assign_task(&expired, &assigner, &executor, now, valid_context()),
            AssignTaskDecision::Reject("expired")
        );

        let mut withdrawn = valid_context();
        withdrawn.offer_hash = None;
        assert_eq!(
            decide_assign_task(&valid, &assigner, &executor, now, withdrawn),
            AssignTaskDecision::Reject("offer_unavailable")
        );

        let mut stale = valid.clone();
        stale.predecessor = Some("c".repeat(64));
        assert_eq!(
            decide_assign_task(&stale, &assigner, &executor, now, valid_context()),
            AssignTaskDecision::Reject("stale_predecessor")
        );

        let mut resolved = valid_context();
        resolved.resolved = true;
        assert_eq!(
            decide_assign_task(&valid, &assigner, &executor, now, resolved),
            AssignTaskDecision::Reject("task_resolved")
        );

        let mut unauthorized = valid_context();
        unauthorized.authorized = false;
        assert_eq!(
            decide_assign_task(&valid, &assigner, &executor, now, unauthorized),
            AssignTaskDecision::Reject("unauthorized")
        );

        let mut duplicate = valid_context();
        duplicate.duplicate = true;
        assert_eq!(
            decide_assign_task(&valid, &assigner, &executor, now, duplicate),
            AssignTaskDecision::Duplicate
        );
    }

    #[test]
    fn typed_assignment_ignores_other_executor_and_rejects_author_mismatch() {
        let executor = public_id('B');
        let other_executor = public_id('C');
        let assigner = public_id('A');
        let attacker = public_id('D');
        let now = Utc::now();

        assert_eq!(
            decide_assign_task(
                &command(&other_executor, &assigner),
                &assigner,
                &executor,
                now,
                valid_context(),
            ),
            AssignTaskDecision::Ignore
        );
        assert_eq!(
            decide_assign_task(
                &command(&executor, &assigner),
                &attacker,
                &executor,
                now,
                valid_context(),
            ),
            AssignTaskDecision::Reject("assigner_mismatch")
        );
    }

    #[test]
    fn duplicate_result_is_trusted_only_when_authored_by_owner() {
        let owner = public_id('B');
        let attacker = public_id('C');
        let assigner = public_id('A');
        let command = command(&owner, &assigner);
        let content =
            serde_json::to_value(AssignTaskResultV1::accepted(&command, "c".repeat(64))).unwrap();
        let message = |author: crate::identity::PublicId| EgregoreMessage {
            author,
            sequence: 1,
            timestamp: Utc::now(),
            content: Some(content.clone()),
            hash: "d".repeat(64),
            signature: "signature".to_string(),
            tags: vec![],
            relates: None,
            trace_id: None,
            span_id: None,
        };

        assert!(!is_recorded_command_result(
            &[message(attacker)],
            &owner,
            &command.command_id,
        ));
        assert!(is_recorded_command_result(
            &[message(owner.clone())],
            &owner,
            &command.command_id,
        ));
    }

    mod feed_disclosure {
        use std::collections::HashSet;

        use serde_json::{json, Value};

        use crate::a2a::A2aPool;
        use crate::authority::Authority;
        use crate::config::{Config, ScopeConfig};
        use crate::egregore::{telemetry, PlannedToolCall, Task, TaskStarted};
        use crate::identity::{Identity, PublicId};
        use crate::mcp::McpPool;
        use crate::runtime::RuntimeContext;
        use crate::scope::ScopeEnforcer;
        use crate::task::{execute_assigned_task, AssignmentDecision, TaskCoordinator};
        use crate::test_support::{assert_same_bytes, CaptureNode, ScriptedClient, SecretGen};

        /// A runtime whose `shell_execute` tool sits under the given block
        /// rules and whose egregore node records every publish.
        async fn scoped_runtime(block: &[&str]) -> (RuntimeContext, CaptureNode) {
            let node = CaptureNode::start().await;
            let mut mcp_pool = McpPool::new();
            mcp_pool.register_test_tool("shell", "execute", ScriptedClient::output("ran"));
            let mut scope_enforcer = ScopeEnforcer::new();
            scope_enforcer
                .add_policy(
                    "shell",
                    &ScopeConfig {
                        allow: vec![],
                        block: block.iter().map(|rule| rule.to_string()).collect(),
                    },
                )
                .unwrap();
            let runtime = RuntimeContext {
                identity: Identity::generate(),
                authority: Authority::insecure_open(),
                mcp_pool,
                a2a_pool: A2aPool::new(),
                scope_enforcer,
                egregore: node.client.clone(),
            };
            (runtime, node)
        }

        /// Run one assigned task carrying `arguments` through the
        /// production assignment handler.
        async fn run_assigned(runtime: &RuntimeContext, arguments: Value) {
            run_call(runtime, "shell_execute", arguments).await;
        }

        /// Run one assigned task with a single planned call.
        async fn run_call(runtime: &RuntimeContext, name: &str, arguments: Value) {
            let requestor = PublicId(format!("@{}.ed25519", "A".repeat(44)));
            let task = Task {
                msg_type: "task".to_string(),
                id: Some("task-1".to_string()),
                hash: "a".repeat(64),
                task_type: Some("shell:execute".to_string()),
                request: None,
                requestor: Some(requestor.clone()),
                prompt: String::new(),
                required_caps: vec!["shell".to_string()],
                parent_id: None,
                context: Default::default(),
                priority: 0,
                timeout_secs: Some(60),
                author: None,
                keeper: None,
                scope_override: None,
                tool_calls: vec![PlannedToolCall {
                    id: "call-1".to_string(),
                    name: name.to_string(),
                    arguments,
                }],
                depends_on: vec![],
                trace_parent: None,
            };
            let assigned = AssignmentDecision {
                task,
                requestor,
                started: TaskStarted::new("task-1", runtime.identity.public_id(), 60),
            };
            let config = Config::from_str("").unwrap();
            let mut coordinator =
                TaskCoordinator::new(runtime.identity.public_id(), config.task.clone());
            execute_assigned_task(
                assigned,
                runtime,
                &config,
                None,
                &mut coordinator,
                &HashSet::new(),
            )
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn scope_rejection_does_not_publish_the_denied_command() {
            let (runtime, node) = scoped_runtime(&["execute:mysql *"]).await;
            let canary = SecretGen::CANARY;

            run_assigned(
                &runtime,
                json!({ "command": format!("mysql -u root -p{canary} -e 'select 1'") }),
            )
            .await;

            assert_eq!(node.bodies_of_type("task_failed").len(), 1);
            for body in node.bodies() {
                let body = String::from_utf8_lossy(&body);
                assert!(
                    !body.contains(canary),
                    "denied command reached the feed: {body}"
                );
            }
        }

        #[tokio::test]
        async fn scope_rejection_names_the_tool_and_rule_for_the_requester() {
            let (runtime, node) = scoped_runtime(&["execute:rm *", "execute:mysql *"]).await;

            run_assigned(&runtime, json!({ "command": "mysql -phunter2" })).await;

            let failed = node.bodies_of_type("task_failed");
            let body: Value = serde_json::from_slice(&failed[0]).unwrap();
            let content = &body["content"];
            assert_eq!(content["reason"], "execution_error");
            assert_eq!(content["code"], "scope_blocked");
            assert_eq!(
                content["details"],
                "tool call 0 (shell:execute) blocked by policy block rule 1"
            );
        }

        #[tokio::test]
        async fn unknown_tool_rejection_does_not_echo_the_call() {
            let (runtime, node) = scoped_runtime(&[]).await;
            let canary = SecretGen::CANARY;

            run_call(
                &runtime,
                &format!("shell_{canary}"),
                json!({ "command": canary }),
            )
            .await;

            let failed = node.bodies_of_type("task_failed");
            let body: Value = serde_json::from_slice(&failed[0]).unwrap();
            assert_eq!(body["content"]["code"], "unknown_tool");
            assert_eq!(
                body["content"]["details"],
                "tool call 0 names no registered tool"
            );
            for body in node.bodies() {
                let body = String::from_utf8_lossy(&body);
                assert!(!body.contains(canary), "{body}");
            }
        }

        /// Information-flow property: with the task's public metadata held
        /// fixed, no change to its arguments changes the published
        /// `task_failed` bytes.
        #[tokio::test]
        async fn scope_rejection_failure_bytes_ignore_the_arguments() {
            let (runtime, node) = scoped_runtime(&["execute:*"]).await;
            let at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc);
            let mut reference: Option<Vec<u8>> = None;

            for seed in 0..64 {
                node.clear();
                let _frozen = telemetry::freeze(at);
                run_assigned(&runtime, SecretGen::new(seed).arguments()).await;

                let failed = node.bodies_of_type("task_failed");
                assert_eq!(failed.len(), 1, "seed {seed}");
                let body = &failed[0];
                match &reference {
                    None => reference = Some(body.clone()),
                    Some(expected) => assert_same_bytes(expected, body, &format!("seed {seed}")),
                }
            }

            // Every body equals the reference, so checking it covers all.
            let reference = String::from_utf8(reference.unwrap()).unwrap();
            assert!(!reference.contains(SecretGen::CANARY), "{reference}");
        }
    }
}
