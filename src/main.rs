//! Servitor — Egregore network task executor.

use std::path::PathBuf;

use chrono::Utc;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use servitor::agent::{create_provider, AgentExecutor};
use servitor::authority::{AuthRequest, Authority, PersonId};
use servitor::comms::discord::DiscordTransport;
use servitor::comms::{CommsResponse, CommsTransport};
use servitor::config::Config;
use servitor::egregore::{EgregoreClient, ScopeConstraints, ServitorProfile, Task, TaskClaim};
use servitor::error::Result;
use servitor::events::cron::CronSource;
use servitor::events::sse::SseSource;
use servitor::events::EventRouter;
use servitor::group::ConsumerGroupCoordinator;
use servitor::identity::Identity;
use servitor::mcp::McpPool;
use servitor::scope::ScopeEnforcer;

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

    let heartbeat_interval_secs = effective_heartbeat_interval_secs(config);
    let heartbeat_interval = std::time::Duration::from_secs(heartbeat_interval_secs);

    // Build event router for non-comms sources
    let mut event_router = EventRouter::new();

    // Add cron source if we have scheduled tasks
    if !config.schedule.is_empty() {
        let cron_source = CronSource::new(&config.schedule)?;
        event_router.add_source(Box::new(cron_source));
        tracing::info!(tasks = config.schedule.len(), "cron source enabled");
    }

    // Add SSE source if subscribe is enabled
    if config.egregore.subscribe {
        let capabilities = mcp_pool.capabilities();
        let heartbeat_interval_ms = heartbeat_interval_secs.saturating_mul(1000);
        let profile = build_profile(&identity, &mcp_pool, config, heartbeat_interval_ms);
        let mut sse_source = SseSource::new(&config.egregore.api_url, capabilities);

        if let Some(ref group_config) = config.egregore.group {
            let mut consumer_group =
                ConsumerGroupCoordinator::new(&group_config.name, identity.public_id());
            consumer_group.observe_profile(&profile, Utc::now());
            sse_source = sse_source.with_consumer_group(consumer_group);
        }

        event_router.add_source(Box::new(sse_source));
        tracing::info!("SSE subscription enabled");
    }

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
    let profile = build_profile(
        &identity,
        &mcp_pool,
        config,
        heartbeat_interval_secs.saturating_mul(1000),
    );
    if let Err(e) = egregore.publish_profile(&profile).await {
        tracing::warn!(error = %e, "failed to publish profile (egregore may be offline)");
    }

    // Main event loop
    let poll_interval = std::time::Duration::from_millis(100);
    let mut last_heartbeat = std::time::Instant::now();

    tracing::info!(
        sources = event_router.source_count(),
        discord = discord_transport.is_some(),
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

                    // Claim and execute
                    let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
                    let _ = egregore.publish_claim(&claim).await;

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
                    let profile = build_profile(
                        &identity,
                        &mcp_pool,
                        config,
                        heartbeat_interval_secs.saturating_mul(1000),
                    );
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
        hash: hash_str,
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
        hash: format!("{:x}", md5_hash(prompt)),
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
    if let Some(ref group_config) = config.egregore.group {
        println!(
            "Consumer Group: {} (heartbeat {}s)",
            group_config.name, group_config.heartbeat_interval_secs
        );
    }
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
fn build_profile(
    identity: &Identity,
    mcp_pool: &McpPool,
    config: &Config,
    heartbeat_interval_ms: u64,
) -> ServitorProfile {
    let mut profile = ServitorProfile::new(identity.public_id(), heartbeat_interval_ms);

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

    if let Some(ref group_config) = config.egregore.group {
        profile.groups.push(group_config.name.clone());
    }

    profile
}

fn effective_heartbeat_interval_secs(config: &Config) -> u64 {
    match config.egregore.group.as_ref() {
        Some(group_config) => config
            .heartbeat
            .interval_secs
            .min(group_config.heartbeat_interval_secs),
        None => config.heartbeat.interval_secs,
    }
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
    fn consumer_group_uses_tighter_heartbeat() {
        let config = Config::from_str(
            r#"
[egregore]
subscribe = true

[egregore.group]
name = "workers"
heartbeat_interval_secs = 5

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

[heartbeat]
interval_secs = 10
"#,
        )
        .unwrap();

        assert_eq!(effective_heartbeat_interval_secs(&config), 5);
    }

    #[test]
    fn build_profile_includes_consumer_group_membership() {
        let config = Config::from_str(
            r#"
[egregore]
subscribe = true

[egregore.group]
name = "workers"

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
"#,
        )
        .unwrap();
        let identity = Identity::generate();
        let pool = McpPool::new();

        let profile = build_profile(&identity, &pool, &config, 5_000);

        assert_eq!(profile.groups, vec!["workers".to_string()]);
        assert_eq!(profile.heartbeat_interval_ms, 5_000);
    }
}
