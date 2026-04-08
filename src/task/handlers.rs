//! Task message handlers for the daemon event loop.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::a2a::A2aPool;
use crate::agent::AgentExecutor;
use crate::authority::{Authority, PersonId};
use crate::config::Config;
use crate::egregore::{
    AuthGate, EgregoreClient, EgregoreMessage, TaskAssign, TaskFailed, TaskFailureReason, TaskPing,
    TaskStatusMessage,
};
use crate::error::Result;
use crate::events::sse::SseSource;
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::runtime::publish_auth_denied_event;
use crate::scope::ScopeEnforcer;
use crate::task::{
    assign_skill, authorize_assignment, authorize_offer_request, request_skill,
    task_matches_capabilities, AssignmentDecision, TaskCoordinator, TaskLifecycleEvent,
};

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

        let Some(requestor) = task.requestor.clone() else {
            tracing::warn!(task_id = %task.effective_id(), "ignoring task without requestor");
            return Ok(None);
        };

        if requestor != message.author {
            tracing::warn!(
                task_id = %task.effective_id(),
                author = %message.author,
                "ignoring task with mismatched requestor and envelope author"
            );
            return Ok(None);
        }

        let auth_result = authorize_offer_request(authority, &requestor, &task);

        if !auth_result.allowed {
            publish_auth_denied_event(
                egregore,
                identity,
                &PersonId::from_egregore(requestor.0.clone()),
                &request_skill(&task),
                AuthGate::Offer,
                &auth_result.reason,
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

        let offer = task_coordinator
            .register_offer(task, requestor, capability_set.iter().cloned().collect())
            .offer;
        egregore.publish_offer(&offer).await?;

        return Ok(None);
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
            &PersonId::from_egregore(assigner.0.clone()),
            &assign_skill(&task),
            AuthGate::Assignment,
            "assignment authorization denied",
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
#[allow(clippy::too_many_arguments)]
pub async fn execute_assigned_task(
    assigned: AssignmentDecision,
    provider: &dyn crate::agent::provider::Provider,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    scope_enforcer: &ScopeEnforcer,
    identity: &Identity,
    authority: &Authority,
    egregore: &EgregoreClient,
    config: &Config,
    mut sse_source: Option<&mut SseSource>,
    task_coordinator: &mut TaskCoordinator,
    capability_set: &HashSet<String>,
) -> Result<()> {
    let task_id = assigned.task.effective_id().to_string();
    let servitor_id = identity.public_id();
    let eta_seconds = assigned.started.eta_seconds;

    egregore.publish_started(&assigned.started).await?;

    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(eta_seconds);
    let executor = AgentExecutor::new(provider, mcp_pool, scope_enforcer, identity, &config.agent)
        .with_egregore(egregore)
        .with_a2a_pool(a2a_pool)
        .with_authority(authority, assigned.task.keeper.clone());
    let execution = executor.execute(&assigned.task);
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
                        egregore.publish_result(&task_result).await?;
                    }
                    Err(error) => {
                        let _ = task_coordinator.finish_execution(&task_id);
                        let failed = TaskFailed::new(
                            task_id.clone(),
                            servitor_id.clone(),
                            TaskFailureReason::ExecutionError,
                            Some(error.to_string()),
                        );
                        egregore.publish_failed(&failed).await?;
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
                        TaskFailureReason::Timeout,
                        Some(format!("task exceeded {}s execution timeout", eta_seconds)),
                    );
                    egregore.publish_failed(&failed).await?;
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
                                egregore.publish_status(&status).await?;
                                continue;
                            }
                        }

                        if message.as_task().is_some() || message.as_task_assign().is_some() {
                            if let Some(assigned) = process_sse_message(
                                &message,
                                authority,
                                identity,
                                capability_set,
                                egregore,
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
                            let _ = egregore.publish_offer_withdraw(&withdraw).await;
                        }
                        TaskLifecycleEvent::Failed(failed) => {
                            let _ = egregore.publish_failed(&failed).await;
                        }
                    }
                }
            }
        }
    }
}
