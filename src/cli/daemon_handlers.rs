//! Extracted handler functions for the daemon event loop.
//!
//! These functions handle specific event types to keep the main loop manageable.

use std::time::Instant;

use crate::a2a::A2aPool;
use crate::agent::provider::Provider;
use crate::agent::AgentExecutor;
use crate::authority::{AuthRequest, Authority, PersonId};
use crate::comms::{task_from_comms, CommsMessage, CommsResponder, CommsResponse};
use crate::config::Config;
use crate::egregore::{build_profile, AuthGate, EgregoreClient, Task, TaskClaim, TaskStatus};
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::metrics::{self, AuthDecision};
use crate::runtime::{publish_auth_denied_event, RuntimeStats};
use crate::scope::ScopeEnforcer;

/// Result of handling a Discord message.
pub enum DiscordHandleResult {
    /// Message was unauthorized, already handled
    Unauthorized,
    /// Message was processed successfully
    Processed,
}

/// Handle an incoming Discord message.
///
/// Authorizes the user, executes the task, and sends the response.
#[allow(clippy::too_many_arguments)]
pub async fn handle_discord_message(
    comms_msg: CommsMessage,
    responder: Box<dyn CommsResponder>,
    authority: &Authority,
    identity: &Identity,
    egregore: &EgregoreClient,
    runtime_stats: &mut RuntimeStats,
    provider: &dyn Provider,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    scope_enforcer: &ScopeEnforcer,
    config: &Config,
) -> DiscordHandleResult {
    tracing::info!(
        source = %comms_msg.source.name(),
        user = %comms_msg.user_name,
        "received comms message"
    );
    runtime_stats.record_task_offer();

    // Authorize the Discord user
    let person = PersonId::from_discord(&comms_msg.user_id);
    let guild_id = match &comms_msg.source {
        crate::comms::CommsSource::Discord { guild_id, .. } => guild_id.clone(),
        _ => "dm".to_string(),
    };
    let place = format!("discord:{}:{}", guild_id, comms_msg.channel_id);
    let auth_result = authority.authorize(&AuthRequest {
        person: person.clone(),
        place: place.clone(),
        skill: "*".to_string(),
    });

    if !auth_result.allowed {
        publish_auth_denied_event(
            egregore,
            identity,
            &person,
            &place,
            "*",
            AuthGate::Offer,
            &auth_result.reason,
        )
        .await;
        tracing::info!(
            user = %comms_msg.user_id,
            reason = %auth_result.reason,
            "ignoring unauthorized Discord message"
        );
        runtime_stats.discard_task();
        // Send rejection message
        let comms_response = CommsResponse {
            channel_id: comms_msg.channel_id.clone(),
            reply_to: Some(comms_msg.message_id.clone()),
            content: "You are not authorized to use this Servitor.".to_string(),
        };
        let _ = responder.send(comms_response).await;
        return DiscordHandleResult::Unauthorized;
    }

    let keeper_name = auth_result.keeper.clone();
    if let Some(ref name) = keeper_name {
        tracing::debug!(keeper = %name, "authorized as keeper");
    }
    metrics::record_auth_decision(AuthDecision::Allowed);

    // Build task from comms message
    let mut task = task_from_comms(&comms_msg);
    task.keeper = keeper_name.clone();
    runtime_stats.start_task();

    // Execute
    let executor = AgentExecutor::new(provider, mcp_pool, scope_enforcer, identity, &config.agent)
        .with_egregore(egregore)
        .with_a2a_pool(a2a_pool)
        .with_authority(authority, keeper_name);

    match executor.execute(&task).await {
        Ok(result) => {
            // Extract text response
            let response_text = result
                .result
                .as_ref()
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| result.result.as_ref().map(|v| v.to_string()))
                .unwrap_or_else(|| "Task completed.".to_string());

            // Send response back to comms
            let comms_response = CommsResponse {
                channel_id: comms_msg.channel_id.clone(),
                reply_to: Some(comms_msg.message_id.clone()),
                content: response_text,
            };

            if let Err(e) = responder.send(comms_response).await {
                tracing::error!(error = %e, "failed to send comms response");
            }

            // Also publish to egregore
            if let Err(e) = egregore.publish_result(&result).await {
                tracing::debug!(error = %e, "failed to publish result to egregore");
            }

            runtime_stats.finish_task(
                matches!(result.status, TaskStatus::Success),
                task.task_type.as_deref(),
            );
            tracing::info!(status = ?result.status, "comms task complete");
        }
        Err(e) => {
            runtime_stats.finish_task(false, task.task_type.as_deref());
            tracing::error!(error = %e, "comms task execution failed");

            // Send error back to user
            let comms_response = CommsResponse {
                channel_id: comms_msg.channel_id.clone(),
                reply_to: Some(comms_msg.message_id.clone()),
                content: format!("Error: {}", e),
            };
            let _ = responder.send(comms_response).await;
        }
    }

    DiscordHandleResult::Processed
}

/// Authorize and execute a task from the event router (cron or SSE).
#[allow(clippy::too_many_arguments)]
pub async fn handle_event_router_task(
    mut task: Task,
    authority: &Authority,
    identity: &Identity,
    egregore: &EgregoreClient,
    runtime_stats: &mut RuntimeStats,
    provider: &dyn Provider,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    scope_enforcer: &ScopeEnforcer,
    config: &Config,
) {
    // Authorize task if it has an author (from SSE)
    let keeper_name = if let Some(ref author) = task.author {
        let person = PersonId::from_egregore(author);
        let auth_result = authority.authorize(&AuthRequest {
            person: person.clone(),
            place: "egregore:local".to_string(),
            skill: "*".to_string(),
        });

        if !auth_result.allowed {
            publish_auth_denied_event(
                egregore,
                identity,
                &person,
                "egregore:local",
                "*",
                AuthGate::Offer,
                &auth_result.reason,
            )
            .await;
            tracing::info!(
                author = %author,
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
    let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
    let _ = egregore.publish_claim(&claim).await;

    let executor = AgentExecutor::new(provider, mcp_pool, scope_enforcer, identity, &config.agent)
        .with_egregore(egregore)
        .with_a2a_pool(a2a_pool)
        .with_authority(authority, keeper_name);

    match executor.execute(&task).await {
        Ok(result) => {
            let should_publish = task
                .context
                .get("publish")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if should_publish {
                if let Err(e) = egregore.publish_result(&result).await {
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
pub async fn handle_heartbeat(
    identity: &Identity,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    config: &Config,
    runtime_stats: &RuntimeStats,
    egregore: &EgregoreClient,
    last_heartbeat: &mut Instant,
) {
    let profile = build_profile(identity, mcp_pool, a2a_pool, config, runtime_stats).await;
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
                if let Err(e) = egregore.publish_failed(&failed).await {
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
