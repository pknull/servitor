//! Extracted handler functions for the daemon event loop.
//!
//! These functions handle specific event types to keep the main loop manageable.

use std::time::Instant;

use crate::a2a::A2aPool;
use crate::authority::PersonId;
use crate::config::Config;
use crate::egregore::{build_profile, AuthGate, EgregoreClient, Task, TaskClaim, TaskStatus};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::runtime::{publish_auth_denied_event, RuntimeContext, RuntimeStats};
use crate::task::{authorize_offer_request, request_skill};

/// Authorize and execute a task from the event router (cron or SSE).
pub async fn handle_event_router_task(
    mut task: Task,
    ctx: &RuntimeContext,
    runtime_stats: &mut RuntimeStats,
    config: &Config,
) {
    // Authorize task if it has an author (from SSE)
    let keeper_name = if let Some(ref author) = task.author {
        let person = PersonId::from_egregore(author);
        let requestor = task
            .requestor
            .clone()
            .unwrap_or_else(|| crate::identity::PublicId(author.clone()));
        let auth_result = authorize_offer_request(&ctx.authority, &requestor, &task);

        if !auth_result.allowed {
            publish_auth_denied_event(
                &ctx.egregore,
                &ctx.identity,
                &person,
                &request_skill(&task),
                AuthGate::Offer,
                &auth_result.reason,
            )
            .await;
            tracing::info!(
                author = %author,
                task_type = %task.effective_task_type(),
                reason = %auth_result.reason,
                "skipping unauthorized task"
            );
            runtime_stats.discard_task();
            return;
        }

        if let Some(ref name) = auth_result.keeper {
            tracing::debug!(keeper = %name, "authorized as keeper");
        }
        auth_result.keeper
    } else {
        // No author (e.g., cron task) - no keeper restriction
        None
    };

    // Set keeper on task for downstream use
    task.keeper = keeper_name.clone();
    runtime_stats.start_task();

    // Claim and execute
    let claim = TaskClaim::new(task.hash.clone(), ctx.identity.public_id(), 180);
    let _ = ctx.egregore.publish_claim(&claim).await;

    // Direct execution only — tasks must have pre-planned tool_calls
    if !task.is_direct() {
        tracing::warn!(
            task_hash = %task.hash,
            "rejecting task without tool_calls — servitors require pre-planned tool calls"
        );
        let _ =
            crate::task::publish_missing_tool_calls_rejection(&ctx.egregore, &ctx.identity, &task)
                .await;
        runtime_stats.finish_task(false, task.task_type.as_deref());
        return;
    }

    tracing::info!(
        task_hash = %task.hash,
        tool_count = task.tool_calls.len(),
        "executing direct tool calls"
    );
    let execution_result = crate::agent::direct::execute_direct(
        &task,
        &ctx.mcp_pool,
        &ctx.scope_enforcer,
        &ctx.identity,
        &config.agent,
        Some(&ctx.egregore),
        Some(&ctx.authority),
        keeper_name.as_deref(),
    )
    .await;

    match execution_result {
        Ok(result) => {
            let should_publish = task
                .context
                .get("publish")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if should_publish {
                if let Err(e) = ctx.egregore.publish_result(&result).await {
                    tracing::warn!(error = %e, "failed to publish result");
                }
            }

            runtime_stats.finish_task(
                matches!(result.status, TaskStatus::Success),
                task.task_type.as_deref(),
            );
            tracing::info!(
                status = ?result.status,
                hash = %result.result_hash,
                "task complete"
            );
        }
        Err(e) => {
            runtime_stats.finish_task(false, task.task_type.as_deref());
            tracing::error!(error = %e, "task execution failed");
        }
    }
}

/// Publish a heartbeat profile to egregore.
#[allow(clippy::too_many_arguments)]
pub async fn handle_heartbeat(
    identity: &Identity,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    config: &Config,
    runtime_stats: &RuntimeStats,
    egregore: &EgregoreClient,
    manifest_ref: Option<&str>,
    last_heartbeat: &mut Instant,
) {
    let profile = build_profile(
        identity,
        mcp_pool,
        a2a_pool,
        config,
        runtime_stats,
        manifest_ref,
    )
    .await;
    if let Err(e) = egregore.publish_profile(&profile).await {
        tracing::debug!(error = %e, "heartbeat failed");
    } else {
        tracing::debug!("heartbeat published");
    }
    *last_heartbeat = Instant::now();
}

/// Process task lifecycle timeout events.
pub async fn handle_lifecycle_timeouts(
    events: Vec<crate::task::TaskLifecycleEvent>,
    egregore: &EgregoreClient,
) {
    use crate::task::TaskLifecycleEvent;

    for event in events {
        match event {
            TaskLifecycleEvent::Withdraw(withdraw) => {
                if let Err(e) = egregore.publish_offer_withdraw(&withdraw).await {
                    tracing::debug!(error = %e, task_id = %withdraw.task_id, "failed to publish offer withdrawal");
                }
            }
            TaskLifecycleEvent::Failed(failed) => {
                if let Err(e) = egregore.publish_failed(&failed, None, None).await {
                    tracing::debug!(error = %e, task_id = %failed.task_id, "failed to publish task failure");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Handler tests would require significant mocking infrastructure.
    // The handlers are integration-tested through the daemon tests.
}
