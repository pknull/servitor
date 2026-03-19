//! Daemon mode implementation for the servitor event loop.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::a2a::server as a2a_server;
use crate::comms::discord::DiscordTransport;
use crate::comms::CommsTransport;
use crate::config::Config;
use crate::egregore::build_profile;
use crate::error::Result;
use crate::events::cron::CronSource;
use crate::events::sse::SseSource;
use crate::events::EventRouter;
use crate::metrics;
use crate::runtime::{RuntimeContext, RuntimeStats};
use crate::task::{execute_assigned_task, process_sse_message, TaskCoordinator};

use super::daemon_handlers::{
    handle_discord_message, handle_event_router_task, handle_heartbeat, handle_lifecycle_timeouts,
};

/// Run servitor as a long-lived daemon with event router.
pub async fn run_daemon(config: &Config, insecure: bool) -> Result<()> {
    // Check if reasoning capability is needed
    let needs_llm =
        config.egregore.subscribe || config.comms.discord.is_some() || !config.schedule.is_empty();

    if needs_llm && config.llm.is_none() {
        return Err(crate::error::ServitorError::Config {
            reason: "Daemon mode with SSE subscribe, Discord, or scheduled tasks requires [llm] configuration. \
                    For worker-only mode (A2A server), disable subscribe and remove comms/schedule sections.".into(),
        });
    }

    // Initialize metrics if enabled
    metrics::init(&config.metrics)?;

    // Initialize core components
    let ctx = RuntimeContext::new(config, insecure).await?;
    metrics::set_mcp_servers_connected(ctx.mcp_pool.capabilities().len() as u64);

    tracing::info!(id = %ctx.identity.public_id(), "starting daemon mode");

    // Build event router for non-network sources
    let mut event_router = EventRouter::new();
    if !config.schedule.is_empty() {
        let cron_source = CronSource::new(&config.schedule)?;
        event_router.add_source(Box::new(cron_source));
        tracing::info!(tasks = config.schedule.len(), "cron source enabled");
    }

    // Initialize SSE source if subscribed
    // Capability set includes both MCP servers and A2A agents
    let mut capability_set: HashSet<String> = ctx.mcp_pool.capabilities().into_iter().collect();
    capability_set.extend(ctx.a2a_pool.agents());
    let mut sse_source = if config.egregore.subscribe {
        tracing::info!("SSE subscription enabled");
        Some(SseSource::new(
            &config.egregore.api_url,
            capability_set.iter().cloned().collect(),
        ))
    } else {
        None
    };
    let mut task_coordinator = TaskCoordinator::new(ctx.identity.public_id(), config.task.clone());

    // Initialize Discord transport
    let mut discord_transport = init_discord_transport(config).await;

    // Spawn A2A server if enabled
    let _a2a_server_handle = if let Some(ref a2a_server_config) = config.a2a_server {
        if a2a_server_config.enabled {
            // Create independent pools for the A2A server
            // These are used for tool introspection and execution
            let mcp_pool_shared = Arc::new(RwLock::new(crate::mcp::McpPool::from_config(config)?));
            // Initialize the shared MCP pool
            {
                let mut pool = mcp_pool_shared.write().await;
                pool.initialize_all().await?;
            }

            let a2a_pool_shared = Arc::new(RwLock::new(
                crate::a2a::A2aPool::from_config(config).map_err(|e| {
                    crate::error::ServitorError::Config {
                        reason: format!("failed to create A2A pool for server: {}", e),
                    }
                })?,
            ));
            // Initialize the shared A2A pool
            {
                let mut pool = a2a_pool_shared.write().await;
                if !pool.is_empty() {
                    let _ = pool.initialize_all().await;
                }
            }

            // Create independent scope enforcer for A2A server
            let mut scope_enforcer = crate::scope::ScopeEnforcer::new();
            for (name, mcp_config) in &config.mcp {
                scope_enforcer.add_policy(name, &mcp_config.scope)?;
            }
            for (name, a2a_config) in &config.a2a {
                scope_enforcer.add_policy(name, &a2a_config.scope)?;
            }

            let authority_shared = Arc::new(ctx.authority.clone());
            let scope_enforcer_shared = Arc::new(scope_enforcer);

            match a2a_server::spawn_server(
                a2a_server_config.clone(),
                mcp_pool_shared,
                a2a_pool_shared,
                authority_shared,
                scope_enforcer_shared,
            )
            .await
            {
                Ok(handle) => Some(handle),
                Err(e) => {
                    tracing::error!(error = %e, "failed to spawn A2A server");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Publish initial profile
    let mut runtime_stats = RuntimeStats::new();
    let profile = build_profile(
        &ctx.identity,
        &ctx.mcp_pool,
        &ctx.a2a_pool,
        config,
        &runtime_stats,
    )
    .await;
    if let Err(e) = ctx.egregore.publish_profile(&profile).await {
        tracing::warn!(error = %e, "failed to publish profile (egregore may be offline)");
    }

    // Main event loop
    let heartbeat_interval = std::time::Duration::from_secs(config.heartbeat.interval_secs);
    let poll_interval = std::time::Duration::from_millis(100);
    let mut last_heartbeat = Instant::now();

    tracing::info!(
        sources = event_router.source_count(),
        discord = discord_transport.is_some(),
        sse = sse_source.is_some(),
        a2a_server = _a2a_server_handle.is_some(),
        "entering event loop"
    );

    loop {
        tokio::select! {
            // Handle Discord messages
            Some((comms_msg, responder)) = async {
                if let Some(ref mut transport) = discord_transport {
                    transport.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                // Safety: LLM is required for Discord mode, validated at startup
                let provider = ctx.provider.as_ref()
                    .expect("LLM provider required for Discord mode")
                    .as_ref();
                handle_discord_message(
                    comms_msg,
                    responder,
                    &ctx.authority,
                    &ctx.identity,
                    &ctx.egregore,
                    &mut runtime_stats,
                    provider,
                    &ctx.mcp_pool,
                    &ctx.a2a_pool,
                    &ctx.scope_enforcer,
                    config,
                ).await;
            }

            // Poll other event sources
            _ = tokio::time::sleep(poll_interval) => {
                // Process SSE messages
                if let Some(ref mut source) = sse_source {
                    if let Some(message) = source.next_message().await {
                        match process_sse_message(
                            &message,
                            &ctx.authority,
                            &ctx.identity,
                            &capability_set,
                            &ctx.egregore,
                            &mut task_coordinator,
                            config,
                        ).await {
                            Ok(Some(assigned)) => {
                                // Safety: LLM is required for SSE mode, validated at startup
                                let provider = ctx.provider.as_ref()
                                    .expect("LLM provider required for SSE mode")
                                    .as_ref();
                                if let Err(e) = execute_assigned_task(
                                    assigned,
                                    provider,
                                    &ctx.mcp_pool,
                                    &ctx.a2a_pool,
                                    &ctx.scope_enforcer,
                                    &ctx.identity,
                                    &ctx.authority,
                                    &ctx.egregore,
                                    config,
                                    sse_source.as_mut(),
                                    &mut task_coordinator,
                                    &capability_set,
                                ).await {
                                    tracing::error!(error = %e, "assigned task execution failed");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to process SSE message");
                            }
                        }
                    }
                }

                // Process task lifecycle timeouts
                let timeout_events = task_coordinator.collect_timeouts(Instant::now());
                handle_lifecycle_timeouts(timeout_events, &ctx.egregore).await;

                // Process queued assignments
                if let Some(assigned) = task_coordinator.take_next_assignment() {
                    // Safety: LLM is required for SSE mode, validated at startup
                    let provider = ctx.provider.as_ref()
                        .expect("LLM provider required for SSE mode")
                        .as_ref();
                    if let Err(e) = execute_assigned_task(
                        assigned,
                        provider,
                        &ctx.mcp_pool,
                        &ctx.a2a_pool,
                        &ctx.scope_enforcer,
                        &ctx.identity,
                        &ctx.authority,
                        &ctx.egregore,
                        config,
                        sse_source.as_mut(),
                        &mut task_coordinator,
                        &capability_set,
                    ).await {
                        tracing::error!(error = %e, "queued assigned task execution failed");
                    }
                }

                // Process event router tasks (cron, etc.)
                if let Some((source_idx, task)) = event_router.poll().await {
                    tracing::debug!(
                        source = source_idx,
                        hash = %task.hash,
                        prompt_len = task.prompt.len(),
                        "processing task from event source"
                    );
                    runtime_stats.record_task_offer();

                    // Safety: LLM is required for cron/SSE tasks, validated at startup
                    let provider = ctx.provider.as_ref()
                        .expect("LLM provider required for event router tasks")
                        .as_ref();
                    handle_event_router_task(
                        task,
                        &ctx.authority,
                        &ctx.identity,
                        &ctx.egregore,
                        &mut runtime_stats,
                        provider,
                        &ctx.mcp_pool,
                        &ctx.a2a_pool,
                        &ctx.scope_enforcer,
                        config,
                    ).await;
                }

                // Check heartbeat
                if last_heartbeat.elapsed() >= heartbeat_interval {
                    handle_heartbeat(
                        &ctx.identity,
                        &ctx.mcp_pool,
                        &ctx.a2a_pool,
                        config,
                        &runtime_stats,
                        &ctx.egregore,
                        &mut last_heartbeat,
                    ).await;
                }
            }
        }
    }
}

/// Initialize Discord transport if configured.
async fn init_discord_transport(config: &Config) -> Option<DiscordTransport> {
    let discord_config = config.comms.discord.as_ref()?;

    match DiscordTransport::new(discord_config) {
        Ok(mut transport) => {
            if let Err(e) = transport.connect().await {
                tracing::error!(error = %e, "failed to connect Discord transport");
                None
            } else {
                tracing::info!("Discord transport connected");
                Some(transport)
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to create Discord transport");
            None
        }
    }
}
