//! Extracted handler functions for the daemon event loop.
//!
//! These functions handle specific event types to keep the main loop manageable.

use std::time::Instant;

use crate::a2a::A2aPool;
use crate::authority::{DenialReason, PersonId};
use crate::config::Config;
use crate::egregore::{build_profile, AuthGate, EgregoreClient, Task, TaskClaim, TaskStatus};
use crate::identity::{Identity, PublicId};
use crate::mcp::McpPool;
use crate::runtime::{publish_auth_denied_event, RuntimeContext, RuntimeStats};
use crate::task::{authorize_offer_request, request_skill, signed_requestor};

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
        let Some(requestor) = signed_requestor(&task, &PublicId(author.clone())) else {
            runtime_stats.discard_task();
            return;
        };
        let auth_result = authorize_offer_request(&ctx.authority, &requestor, &task);

        if !auth_result.allowed {
            publish_auth_denied_event(
                &ctx.egregore,
                &ctx.identity,
                &ctx.authority,
                &person,
                &request_skill(&task),
                AuthGate::Offer,
                auth_result
                    .denial
                    .unwrap_or(DenialReason::NoMatchingPermission),
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
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::authority::{Authority, AuthorityConfig};
    use crate::config::ToolCallTemplate;
    use crate::egregore::{PlannedToolCall, ServitorLoad, ServitorStats};
    use crate::events::task_from_template;
    use crate::scope::ScopeEnforcer;

    const KEEPER: &str = "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519";
    const STRANGER: &str = "@CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=.ed25519";

    /// Upper bound on one test's handler run, so a stall fails instead of hanging CI.
    const TEST_DEADLINE: Duration = Duration::from_secs(10);

    /// Answer every request on `listener` with a 503, so each publish fails at once.
    async fn refuse_requests(listener: TcpListener) {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\n\
                          content-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
                // Drain any body until the client closes, so the socket is not reset.
                while matches!(stream.read(&mut buf).await, Ok(n) if n > 0) {}
            });
        }
    }

    /// A context whose one keeper may request anything. No MCP servers are
    /// configured, so a task that passes intake fails at execution. Egregore is
    /// a loopback listener held for the test that answers 503, so every publish
    /// fails fast.
    async fn test_context() -> RuntimeContext {
        let authority = Authority::from_config(
            AuthorityConfig::from_toml(&format!(
                "[[keeper]]\nname = \"keeper\"\negregore = \"{KEEPER}\"\n\n\
                 [[permission]]\nkeeper = \"keeper\"\nskills = [\"*\"]\n"
            ))
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let egregore_addr = listener.local_addr().unwrap();
        tokio::spawn(refuse_requests(listener));
        RuntimeContext {
            identity: Identity::generate(),
            authority,
            mcp_pool: McpPool::new(),
            a2a_pool: A2aPool::new(),
            scope_enforcer: ScopeEnforcer::new(),
            egregore: EgregoreClient::new(&format!("http://{egregore_addr}")),
        }
    }

    /// A direct task carrying its signed author, as an SSE event source hands it on.
    fn signed_task(author: &str, requestor: &str) -> Task {
        Task {
            msg_type: "task".to_string(),
            id: Some("task-1".to_string()),
            hash: "hash-1".to_string(),
            task_type: Some("shell".to_string()),
            request: Some("pwd".to_string()),
            requestor: Some(PublicId(requestor.to_string())),
            prompt: "pwd".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: Some(60),
            author: Some(author.to_string()),
            keeper: None,
            scope_override: None,
            tool_calls: vec![PlannedToolCall {
                id: "call-1".to_string(),
                name: "shell__execute".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
            }],
            depends_on: vec![],
        }
    }

    /// A task as a cron or MCP-notification template builds it, with no author.
    fn local_template_task() -> Task {
        task_from_template(
            "local",
            None,
            &[ToolCallTemplate {
                name: "shell__execute".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
            }],
            Default::default(),
        )
    }

    /// Hand one queued task to the router handler, within the test deadline,
    /// and return the counters it leaves behind.
    async fn run_router(task: Task) -> (ServitorStats, ServitorLoad) {
        tokio::time::timeout(TEST_DEADLINE, async {
            let ctx = test_context().await;
            let config = Config::from_str("").unwrap();
            let mut stats = RuntimeStats::new();
            stats.record_task_offer();
            handle_event_router_task(task, &ctx, &mut stats, &config).await;
            (stats.stats(), stats.load())
        })
        .await
        .expect("router handler did not finish within the test deadline")
    }

    /// True when the task passed intake and an execution was attempted.
    async fn router_runs(task: Task) -> bool {
        let (stats, _) = run_router(task).await;
        stats.tasks_executed + stats.tasks_failed > 0
    }

    #[tokio::test]
    async fn router_task_requested_by_its_author_runs() {
        assert!(router_runs(signed_task(KEEPER, KEEPER)).await);
    }

    #[tokio::test]
    async fn router_runs_authorless_local_template_task() {
        // No author means a trusted local source: no keeper, scope policy only.
        assert!(router_runs(local_template_task()).await);
    }

    #[tokio::test]
    async fn router_task_with_author_but_no_requestor_is_ignored() {
        let mut task = signed_task(KEEPER, KEEPER);
        task.requestor = None;

        let (stats, load) = run_router(task).await;
        assert_eq!(
            stats.tasks_executed + stats.tasks_failed,
            0,
            "author-bearing task without a requestor passed intake"
        );
        assert_eq!(load.tasks_queued, 0, "discarded task left in the queue");
        assert_eq!(load.tasks_executing, 0, "discarded task left executing");
    }

    #[tokio::test]
    async fn router_task_from_unknown_requestor_is_ignored() {
        assert!(!router_runs(signed_task(STRANGER, STRANGER)).await);
    }

    #[tokio::test]
    async fn router_task_whose_requestor_is_not_its_author_is_ignored() {
        // A stranger signs a task that names the keeper as its requestor.
        assert!(
            !router_runs(signed_task(STRANGER, KEEPER)).await,
            "task ran on the authority of a requestor who did not sign it"
        );
    }
}
