//! Servitor — Egregore network task executor.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use servitor::agent::{create_provider, AgentExecutor};
use servitor::authority::{AuthRequest, Authority, PersonId};
use servitor::comms::discord::DiscordTransport;
use servitor::comms::{CommsResponse, CommsTransport};
use servitor::config::Config;
use servitor::egregore::{
    Attestation, CapabilityChallenge, CapabilityProof, EgregoreClient, EgregoreMessage,
    ScopeConstraints, ServitorProfile, Task, TaskAssign, TaskClaim, TaskFailed, TaskFailureReason,
    TaskPing, TaskStatusMessage,
};
use servitor::error::Result;
use servitor::events::cron::CronSource;
use servitor::events::sse::SseSource;
use servitor::events::EventRouter;
use servitor::identity::Identity;
use servitor::mcp::McpPool;
use servitor::scope::ScopeEnforcer;
use servitor::task::{
    authorize_assignment, authorize_offer_request, AssignmentDecision, TaskCoordinator,
    TaskLifecycleEvent,
};

#[derive(Parser)]
#[command(name = "servitor")]
#[command(about = "Egregore network task executor using MCP servers")]
#[command(version)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "servitor.toml")]
    config: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as a daemon (hook mode or standalone)
    Run {
        /// Run in hook mode (receive task from stdin)
        #[arg(long)]
        hook: bool,
    },

    /// Execute a task directly (for testing)
    Exec {
        /// Task prompt
        prompt: String,
    },

    /// Show identity and capabilities
    Info,

    /// Initialize identity and configuration
    Init {
        /// Force regenerate identity
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    // Load configuration
    let mut config = if cli.config.exists() {
        Config::load(&cli.config)?
    } else if matches!(cli.command, Some(Commands::Init { .. })) {
        // Init doesn't need existing config
        create_default_config()?
    } else {
        return Err(servitor::ServitorError::Config {
            reason: format!("config file not found: {}", cli.config.display()),
        });
    };

    config.expand_paths();

    match cli.command {
        Some(Commands::Run { hook }) => {
            if hook {
                run_hook_mode(&config).await
            } else {
                run_daemon_mode(&config).await
            }
        }
        Some(Commands::Exec { prompt }) => run_exec(&config, &prompt).await,
        Some(Commands::Info) => run_info(&config).await,
        Some(Commands::Init { force }) => run_init(&config, force).await,
        None => {
            // Default to daemon mode
            run_daemon_mode(&config).await
        }
    }
}

/// Run in hook mode — receive task from stdin, execute, publish result.
async fn run_hook_mode(config: &Config) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;

    // Load authority
    let authority_path = identity_dir.join("authority.toml");
    let authority = Authority::load(&authority_path)?;
    if authority.is_open_mode() {
        tracing::debug!("authority: open mode (no restrictions)");
    } else {
        tracing::debug!("authority: loaded from {}", authority_path.display());
    }

    tracing::info!(id = %identity.public_id(), "starting hook mode");

    // Parse incoming message from stdin
    let message = match servitor::egregore::hook::receive_message() {
        Ok(msg) => msg,
        Err(e) => {
            tracing::error!(error = %e, "failed to receive message");
            return Err(e);
        }
    };

    // Check authority (replaces author_allowlist check)
    let person = PersonId::from_egregore(&message.author.0);
    let auth_result = authority.authorize(&AuthRequest {
        person,
        place: "egregore:local".to_string(),
        skill: "*".to_string(), // Task intake doesn't specify skill yet
    });

    if !auth_result.allowed {
        tracing::info!(
            author = %message.author.0,
            reason = %auth_result.reason,
            "ignoring unauthorized message"
        );
        return Ok(());
    }

    if let Some(ref keeper_name) = auth_result.keeper {
        tracing::debug!(keeper = %keeper_name, "authorized as keeper");
    }

    // Extract task from message
    let task = message
        .as_task()
        .ok_or_else(|| servitor::ServitorError::Egregore {
            reason: "message is not a task".into(),
        })?;

    tracing::info!(hash = %task.hash, prompt = %task.prompt, "received task");

    // Initialize components
    let provider = create_provider(&config.llm)?;
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    let mut scope_enforcer = ScopeEnforcer::new();
    for (name, mcp_config) in &config.mcp {
        scope_enforcer.add_policy(name, &mcp_config.scope)?;
    }

    // Publish claim
    let egregore = EgregoreClient::new(&config.egregore.api_url);
    let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
    if let Err(e) = egregore.publish_claim(&claim).await {
        tracing::warn!(error = %e, "failed to publish claim");
        // Continue anyway — claim is advisory
    }

    // Execute task with context fetching and authority
    let executor = AgentExecutor::new(
        provider.as_ref(),
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
    )
    .with_egregore(&egregore)
    .with_authority(&authority, auth_result.keeper.clone());

    let result = executor.execute(&task).await?;

    // Publish result
    egregore.publish_result(&result).await?;

    // Cleanup
    mcp_pool.shutdown_all().await?;

    tracing::info!(
        status = ?result.status,
        hash = %result.result_hash,
        "task complete"
    );

    Ok(())
}

/// Run as a long-lived daemon with event router.
async fn run_daemon_mode(config: &Config) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;

    // Load authority
    let authority_path = identity_dir.join("authority.toml");
    let authority = Authority::load(&authority_path)?;
    if authority.is_open_mode() {
        tracing::debug!("authority: open mode (no restrictions)");
    } else {
        tracing::debug!("authority: loaded from {}", authority_path.display());
    }

    tracing::info!(id = %identity.public_id(), "starting daemon mode");

    // Initialize components
    let provider = create_provider(&config.llm)?;
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    let mut scope_enforcer = ScopeEnforcer::new();
    for (name, mcp_config) in &config.mcp {
        scope_enforcer.add_policy(name, &mcp_config.scope)?;
    }

    let egregore = EgregoreClient::new(&config.egregore.api_url);

    // Build event router for non-network sources
    let mut event_router = EventRouter::new();

    // Add cron source if we have scheduled tasks
    if !config.schedule.is_empty() {
        let cron_source = CronSource::new(&config.schedule)?;
        event_router.add_source(Box::new(cron_source));
        tracing::info!(tasks = config.schedule.len(), "cron source enabled");
    }

    let capability_set: HashSet<String> = mcp_pool.capabilities().into_iter().collect();
    let tool_set: HashSet<String> = mcp_pool
        .all_tools()
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    let mut sse_source = if config.egregore.subscribe {
        tracing::info!("SSE subscription enabled");
        Some(SseSource::new(
            &config.egregore.api_url,
            capability_set.iter().cloned().collect(),
        ))
    } else {
        None
    };
    let mut task_coordinator = TaskCoordinator::new(identity.public_id(), config.task.clone());

    // Initialize comms transports
    let mut discord_transport: Option<DiscordTransport> = None;
    if let Some(ref discord_config) = config.comms.discord {
        match DiscordTransport::new(discord_config) {
            Ok(mut transport) => {
                if let Err(e) = transport.connect().await {
                    tracing::error!(error = %e, "failed to connect Discord transport");
                } else {
                    discord_transport = Some(transport);
                    tracing::info!("Discord transport connected");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to create Discord transport");
            }
        }
    }

    // Publish initial profile
    let profile = build_profile(&identity, &mcp_pool, config);
    if let Err(e) = egregore.publish_profile(&profile).await {
        tracing::warn!(error = %e, "failed to publish profile (egregore may be offline)");
    }

    // Main event loop
    let heartbeat_interval = std::time::Duration::from_secs(config.heartbeat.interval_secs);
    let poll_interval = std::time::Duration::from_millis(100);
    let mut last_heartbeat = std::time::Instant::now();

    tracing::info!(
        sources = event_router.source_count(),
        discord = discord_transport.is_some(),
        sse = sse_source.is_some(),
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
                tracing::info!(
                    source = %comms_msg.source.name(),
                    user = %comms_msg.user_name,
                    "received comms message"
                );

                // Authorize the Discord user
                let person = PersonId::from_discord(&comms_msg.user_id);
                let guild_id = match &comms_msg.source {
                    servitor::comms::CommsSource::Discord { guild_id, .. } => guild_id.clone(),
                    _ => "dm".to_string(),
                };
                let place = format!("discord:{}:{}", guild_id, comms_msg.channel_id);
                let auth_result = authority.authorize(&AuthRequest {
                    person,
                    place,
                    skill: "*".to_string(),
                });

                if !auth_result.allowed {
                    tracing::info!(
                        user = %comms_msg.user_id,
                        reason = %auth_result.reason,
                        "ignoring unauthorized Discord message"
                    );
                    // Send rejection message
                    let comms_response = CommsResponse {
                        channel_id: comms_msg.channel_id.clone(),
                        reply_to: Some(comms_msg.message_id.clone()),
                        content: "You are not authorized to use this Servitor.".to_string(),
                    };
                    let _ = responder.send(comms_response).await;
                    continue;
                }

                let keeper_name = auth_result.keeper.clone();
                if let Some(ref name) = keeper_name {
                    tracing::debug!(keeper = %name, "authorized as keeper");
                }

                // Build task from comms message
                let mut task = task_from_comms(&comms_msg);
                task.keeper = keeper_name.clone();

                // Execute
                let executor = AgentExecutor::new(
                    provider.as_ref(),
                    &mcp_pool,
                    &scope_enforcer,
                    &identity,
                    &config.agent,
                )
                .with_egregore(&egregore)
                .with_authority(&authority, keeper_name);

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

                        tracing::info!(status = ?result.status, "comms task complete");
                    }
                    Err(e) => {
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
            }

            // Poll other event sources
            _ = tokio::time::sleep(poll_interval) => {
                if let Some(ref mut source) = sse_source {
                    if let Some(message) = source.next_message().await {
                        match process_sse_message(
                            &message,
                            &authority,
                            &identity,
                            &capability_set,
                            &tool_set,
                            &egregore,
                            &mut task_coordinator,
                            config,
                        )
                        .await {
                            Ok(Some(assigned)) => {
                                if let Err(e) = execute_assigned_task(
                                    assigned,
                                    provider.as_ref(),
                                    &mcp_pool,
                                    &scope_enforcer,
                                    &identity,
                                    &authority,
                                    &egregore,
                                    config,
                                    sse_source.as_mut(),
                                    &mut task_coordinator,
                                    &capability_set,
                                    &tool_set,
                                )
                                .await
                                {
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

                for event in task_coordinator.collect_timeouts(Instant::now()) {
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

                if let Some(assigned) = task_coordinator.take_next_assignment() {
                    if let Err(e) = execute_assigned_task(
                        assigned,
                        provider.as_ref(),
                        &mcp_pool,
                        &scope_enforcer,
                        &identity,
                        &authority,
                        &egregore,
                        config,
                        sse_source.as_mut(),
                        &mut task_coordinator,
                        &capability_set,
                        &tool_set,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "queued assigned task execution failed");
                    }
                }

                if let Some((source_idx, mut task)) = event_router.poll().await {
                    tracing::info!(
                        source = source_idx,
                        hash = %task.hash,
                        prompt = %task.prompt,
                        "processing task from event source"
                    );

                    // Authorize task if it has an author (from SSE)
                    let keeper_name = if let Some(ref author) = task.author {
                        let person = PersonId::from_egregore(author);
                        let auth_result = authority.authorize(&AuthRequest {
                            person,
                            place: "egregore:local".to_string(),
                            skill: "*".to_string(),
                        });

                        if !auth_result.allowed {
                            tracing::info!(
                                author = %author,
                                reason = %auth_result.reason,
                                "skipping unauthorized task"
                            );
                            continue;
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

                    let executor = AgentExecutor::new(
                        provider.as_ref(),
                        &mcp_pool,
                        &scope_enforcer,
                        &identity,
                        &config.agent,
                    )
                    .with_egregore(&egregore)
                    .with_authority(&authority, keeper_name);

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

                            tracing::info!(
                                status = ?result.status,
                                hash = %result.result_hash,
                                "task complete"
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "task execution failed");
                        }
                    }
                }

                // Check heartbeat
                if last_heartbeat.elapsed() >= heartbeat_interval {
                    let profile = build_profile(&identity, &mcp_pool, config);
                    if let Err(e) = egregore.publish_profile(&profile).await {
                        tracing::debug!(error = %e, "heartbeat failed");
                    } else {
                        tracing::debug!("heartbeat published");
                    }
                    last_heartbeat = std::time::Instant::now();
                }
            }
        }
    }
}

async fn process_sse_message(
    message: &EgregoreMessage,
    authority: &Authority,
    identity: &Identity,
    capability_set: &HashSet<String>,
    tool_set: &HashSet<String>,
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
            authority,
            identity,
            task_coordinator,
            config,
        )
        .await;
    }

    if let Some(challenge) = message.as_capability_challenge() {
        handle_capability_challenge(
            challenge,
            message,
            authority,
            identity,
            capability_set,
            tool_set,
            egregore,
            task_coordinator,
        )
        .await?;
    }

    Ok(None)
}

async fn maybe_accept_assignment(
    assign: TaskAssign,
    message: &EgregoreMessage,
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

async fn execute_assigned_task(
    assigned: AssignmentDecision,
    provider: &dyn servitor::agent::provider::Provider,
    mcp_pool: &McpPool,
    scope_enforcer: &ScopeEnforcer,
    identity: &Identity,
    authority: &Authority,
    egregore: &EgregoreClient,
    config: &Config,
    mut sse_source: Option<&mut SseSource>,
    task_coordinator: &mut TaskCoordinator,
    capability_set: &HashSet<String>,
    tool_set: &HashSet<String>,
) -> Result<()> {
    let task_id = assigned.task.effective_id().to_string();
    let servitor_id = identity.public_id();
    let eta_seconds = assigned.started.eta_seconds;

    egregore.publish_started(&assigned.started).await?;

    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(eta_seconds);
    let executor = AgentExecutor::new(provider, mcp_pool, scope_enforcer, identity, &config.agent)
        .with_egregore(egregore)
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

                        if message.as_task().is_some()
                            || message.as_task_assign().is_some()
                            || message.as_capability_challenge().is_some()
                        {
                            if let Some(assigned) = process_sse_message(
                                &message,
                                authority,
                                identity,
                                capability_set,
                                tool_set,
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

fn task_matches_capabilities(task: &Task, capability_set: &HashSet<String>) -> bool {
    if task.required_caps.is_empty() {
        return true;
    }

    task.required_caps
        .iter()
        .all(|capability| capability_set.contains(capability))
}

async fn handle_capability_challenge(
    challenge: CapabilityChallenge,
    message: &EgregoreMessage,
    authority: &Authority,
    identity: &Identity,
    capability_set: &HashSet<String>,
    tool_set: &HashSet<String>,
    egregore: &EgregoreClient,
    task_coordinator: &TaskCoordinator,
) -> Result<()> {
    if challenge.servitor != identity.public_id() {
        return Ok(());
    }

    if !challenge_is_fresh(&challenge) {
        tracing::debug!(
            challenge_id = %challenge.challenge_id,
            task_id = %challenge.task_id,
            "ignoring expired capability challenge"
        );
        return Ok(());
    }

    if let Some(content_challenger) = &challenge.challenger {
        if content_challenger != &message.author {
            tracing::warn!(
                challenge_id = %challenge.challenge_id,
                author = %message.author,
                challenger = %content_challenger,
                "ignoring capability challenge with mismatched challenger"
            );
            return Ok(());
        }
    }

    let Some(requestor) = task_coordinator.pending_requestor(&challenge.task_id) else {
        tracing::debug!(
            challenge_id = %challenge.challenge_id,
            task_id = %challenge.task_id,
            "ignoring capability challenge for unknown pending task"
        );
        return Ok(());
    };
    let Some(task) = task_coordinator.pending_task(&challenge.task_id) else {
        return Ok(());
    };
    let challenger = challenge
        .challenger
        .clone()
        .unwrap_or_else(|| message.author.clone());

    if !authorize_assignment(authority, &challenger, requestor, task) {
        tracing::info!(
            challenge_id = %challenge.challenge_id,
            task_id = %challenge.task_id,
            challenger = %challenger,
            "ignoring unauthorized capability challenge"
        );
        return Ok(());
    }

    let matched_tools =
        matching_tools_for_capability(&challenge.capability, capability_set, tool_set);
    let verified = !matched_tools.is_empty();
    let details = if verified {
        Some("matched current local tool inventory".to_string())
    } else {
        Some("no currently initialized tool matched the requested capability".to_string())
    };
    let proof = build_capability_proof(identity, &challenge, verified, matched_tools, details);

    egregore.publish_capability_proof(&proof).await?;
    Ok(())
}

fn build_capability_proof(
    identity: &Identity,
    challenge: &CapabilityChallenge,
    verified: bool,
    matched_tools: Vec<String>,
    details: Option<String>,
) -> CapabilityProof {
    let timestamp = Utc::now();
    let servitor = identity.public_id();
    let mut proof = CapabilityProof {
        msg_type: "capability_proof".to_string(),
        challenge_id: challenge.challenge_id.clone(),
        task_id: challenge.task_id.clone(),
        servitor: servitor.clone(),
        capability: challenge.capability.clone(),
        verified,
        matched_tools,
        details,
        attestation: Attestation {
            servitor_id: servitor,
            signature: String::new(),
            timestamp,
        },
        timestamp,
    };
    proof.attestation.signature = identity.sign(proof.signing_payload().as_bytes());
    proof
}

fn challenge_is_fresh(challenge: &CapabilityChallenge) -> bool {
    let ttl_seconds = if challenge.ttl_seconds == 0 {
        30
    } else {
        challenge.ttl_seconds
    };
    let expires_at = challenge.timestamp + chrono::Duration::seconds(ttl_seconds as i64);
    Utc::now() <= expires_at
}

fn matching_tools_for_capability(
    capability: &str,
    capability_set: &HashSet<String>,
    tool_set: &HashSet<String>,
) -> Vec<String> {
    let mut matches: Vec<String> = tool_set
        .iter()
        .filter(|tool| capability_matches(capability, tool))
        .cloned()
        .collect();

    if matches.is_empty() {
        matches.extend(
            capability_set
                .iter()
                .filter(|candidate| capability_matches(capability, candidate))
                .cloned(),
        );
    }

    matches.sort();
    matches.dedup();
    matches
}

fn capability_matches(pattern: &str, candidate: &str) -> bool {
    let normalized_pattern = pattern.replace(':', "_");
    let normalized_candidate = candidate.replace(':', "_");

    if normalized_pattern == normalized_candidate {
        return true;
    }

    if let Some(prefix) = normalized_pattern.strip_suffix("_*") {
        return normalized_candidate == prefix
            || normalized_candidate.starts_with(&format!("{prefix}_"));
    }

    if let Some(prefix) = normalized_pattern.strip_suffix('*') {
        return normalized_candidate.starts_with(prefix);
    }

    normalized_candidate.starts_with(&format!("{normalized_pattern}_"))
}

/// Build a Task from a CommsMessage.
fn task_from_comms(msg: &servitor::comms::CommsMessage) -> Task {
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    let mut hasher = Sha256::new();
    hasher.update(msg.user_id.as_bytes());
    hasher.update(msg.content.as_bytes());
    hasher.update(msg.timestamp.timestamp().to_le_bytes());
    let hash = hasher.finalize();
    let hash_str: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    let mut context = HashMap::new();
    context.insert("source".to_string(), serde_json::json!(msg.source.name()));
    context.insert(
        "user".to_string(),
        serde_json::json!({
            "id": msg.user_id,
            "name": msg.user_name,
        }),
    );
    context.insert("channel".to_string(), serde_json::json!(msg.channel_id));

    Task {
        msg_type: "task".to_string(),
        id: None,
        hash: hash_str,
        task_type: None,
        request: Some(msg.content.clone()),
        requestor: None,
        prompt: msg.content.clone(),
        required_caps: vec![],
        parent_id: msg.reply_to.clone(),
        context,
        priority: 0,
        timeout_secs: None,
        author: None,
        keeper: None,
    }
}

/// Execute a task directly (for testing).
async fn run_exec(config: &Config, prompt: &str) -> Result<()> {
    // Load identity
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;

    tracing::info!(id = %identity.public_id(), "executing task");

    // Initialize components
    let provider = create_provider(&config.llm)?;
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    let mut scope_enforcer = ScopeEnforcer::new();
    for (name, mcp_config) in &config.mcp {
        scope_enforcer.add_policy(name, &mcp_config.scope)?;
    }

    // Build a task
    let task = servitor::egregore::Task {
        msg_type: "task".to_string(),
        id: None,
        hash: format!("{:x}", md5_hash(prompt)),
        task_type: None,
        request: Some(prompt.to_string()),
        requestor: None,
        prompt: prompt.to_string(),
        required_caps: vec![],
        parent_id: None,
        context: std::collections::HashMap::new(),
        priority: 0,
        timeout_secs: Some(config.agent.timeout_secs),
        author: None,
        keeper: None,
    };

    // Execute (no egregore context for direct exec)
    let executor = AgentExecutor::new(
        provider.as_ref(),
        &mcp_pool,
        &scope_enforcer,
        &identity,
        &config.agent,
    );

    let result = executor.execute(&task).await?;

    // Print result
    println!("Status: {:?}", result.status);
    if let Some(ref r) = result.result {
        println!(
            "Result: {}",
            serde_json::to_string_pretty(r).unwrap_or_default()
        );
    }
    if let Some(ref e) = result.error {
        println!("Error: {}", e);
    }

    // Cleanup
    mcp_pool.shutdown_all().await?;

    Ok(())
}

/// Show identity and capabilities.
async fn run_info(config: &Config) -> Result<()> {
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;

    // Load authority
    let authority_path = identity_dir.join("authority.toml");
    let authority = Authority::load(&authority_path)?;

    println!("Identity: {}", identity.public_id());
    println!("Data dir: {}", config.identity.data_dir);
    println!();

    // Show authority status
    if authority.is_open_mode() {
        println!("Authority: OPEN MODE (no restrictions)");
        println!("  No authority.toml found at {}", authority_path.display());
        println!("  Copy authority.example.toml to enable access control.");
    } else {
        println!("Authority: RESTRICTED");
        println!("  File: {}", authority_path.display());
    }
    println!();

    println!("LLM Provider: {}", config.llm.provider);
    println!("LLM Model: {}", config.llm.model);
    println!();
    println!("MCP Servers:");
    for (name, mcp) in &config.mcp {
        println!("  - {} ({})", name, mcp.transport);
        if !mcp.scope.allow.is_empty() {
            println!("    allow: {:?}", mcp.scope.allow);
        }
        if !mcp.scope.block.is_empty() {
            println!("    block: {:?}", mcp.scope.block);
        }
        if let Some(ref template) = mcp.on_notification {
            println!("    on_notification: {}", template);
        }
    }
    println!();
    println!("Egregore API: {}", config.egregore.api_url);
    println!("SSE Subscribe: {}", config.egregore.subscribe);
    println!();

    if !config.schedule.is_empty() {
        println!("Scheduled Tasks:");
        for task in &config.schedule {
            println!("  - {} ({})", task.name, task.cron);
            println!("    task: {}", task.task);
            if task.publish {
                println!("    publish: true");
            }
        }
    }

    Ok(())
}

/// Initialize identity and configuration.
async fn run_init(config: &Config, force: bool) -> Result<()> {
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let key_path = identity_dir.join("secret.key");

    if key_path.exists() && !force {
        println!("Identity already exists: {}", key_path.display());
        println!("Use --force to regenerate");
        return Ok(());
    }

    if force && key_path.exists() {
        std::fs::remove_file(&key_path)?;
        let pub_path = identity_dir.join("public.key");
        if pub_path.exists() {
            std::fs::remove_file(&pub_path)?;
        }
    }

    let identity = Identity::load_or_generate(&identity_dir)?;
    println!("Identity: {}", identity.public_id());
    println!("Saved to: {}", key_path.display());

    Ok(())
}

/// Build a ServitorProfile for publishing.
fn build_profile(identity: &Identity, mcp_pool: &McpPool, config: &Config) -> ServitorProfile {
    let mut profile =
        ServitorProfile::new(identity.public_id(), config.heartbeat.interval_secs * 1000);

    // Add capabilities from MCP servers
    profile.capabilities = mcp_pool.capabilities();

    // Add tools
    profile.tools = mcp_pool
        .all_tools()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    // Add scopes
    for (name, mcp_config) in &config.mcp {
        profile.scopes.insert(
            name.clone(),
            ScopeConstraints {
                allow: mcp_config.scope.allow.clone(),
                block: mcp_config.scope.block.clone(),
            },
        );
    }

    profile
}

/// Create a default configuration.
fn create_default_config() -> Result<Config> {
    let toml = r#"
[identity]
data_dir = "~/.servitor"

[egregore]
api_url = "http://127.0.0.1:7654"
subscribe = false

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

[agent]
max_turns = 50
timeout_secs = 300

[task]
offer_ttl_secs = 300
offer_timeout_secs = 60
assign_timeout_secs = 300
start_timeout_secs = 30
eta_buffer_multiplier = 1.5
ping_timeout_secs = 30

[heartbeat]
interval_secs = 10
"#;
    Config::from_str(toml)
}

/// Simple hash for task ID generation in exec mode.
fn md5_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_freshness_respects_ttl() {
        let fresh = CapabilityChallenge {
            msg_type: "capability_challenge".to_string(),
            challenge_id: "challenge-1".to_string(),
            task_id: "task-1".to_string(),
            servitor: servitor::PublicId(
                "@SERVITORAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string(),
            ),
            capability: "shell:execute".to_string(),
            challenger: None,
            ttl_seconds: 30,
            timestamp: Utc::now(),
        };
        assert!(challenge_is_fresh(&fresh));

        let stale = CapabilityChallenge {
            timestamp: Utc::now() - chrono::Duration::seconds(61),
            ttl_seconds: 30,
            ..fresh
        };
        assert!(!challenge_is_fresh(&stale));
    }

    #[test]
    fn capability_matching_finds_tools_and_server_caps() {
        let capabilities = HashSet::from(["shell".to_string(), "docker".to_string()]);
        let tools = HashSet::from([
            "shell_execute".to_string(),
            "docker_container_list".to_string(),
        ]);

        assert_eq!(
            matching_tools_for_capability("shell:execute", &capabilities, &tools),
            vec!["shell_execute".to_string()]
        );
        assert_eq!(
            matching_tools_for_capability("docker:*", &capabilities, &tools),
            vec!["docker_container_list".to_string()]
        );
    }

    #[test]
    fn capability_proof_verifies_known_tool() {
        let identity = Identity::generate();
        let challenge = CapabilityChallenge::new("task-1", identity.public_id(), "shell", None, 30);
        let matched_tools = vec!["shell_execute".to_string(), "shell_read".to_string()];
        let proof = build_capability_proof(
            &identity,
            &challenge,
            true,
            matched_tools,
            Some("matched local tools".to_string()),
        );

        assert!(proof.verified);
        assert_eq!(proof.matched_tools.len(), 2);
        assert!(proof.verify().unwrap());
    }

    #[test]
    fn capability_proof_marks_missing_capability_unverified() {
        let identity = Identity::generate();
        let challenge =
            CapabilityChallenge::new("task-1", identity.public_id(), "browser", None, 30);
        let proof = build_capability_proof(
            &identity,
            &challenge,
            false,
            Vec::new(),
            Some("no match".to_string()),
        );

        assert!(!proof.verified);
        assert!(proof.matched_tools.is_empty());
    }
}
