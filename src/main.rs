//! Servitor — Egregore network task executor.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use servitor::agent::{create_provider, AgentExecutor};
use servitor::authority::{AuthRequest, Authority, PersonId};
use servitor::comms::discord::DiscordTransport;
use servitor::comms::{CommsResponse, CommsTransport};
use servitor::config::{Config, DeploymentMode};
use servitor::egregore::{EgregoreClient, ScopeConstraints, ServitorProfile, Task, TaskClaim};
use servitor::error::Result;
use servitor::events::cron::CronSource;
use servitor::events::local::LocalSource;
use servitor::events::sse::SseSource;
use servitor::events::EventRouter;
use servitor::identity::Identity;
use servitor::mcp::McpPool;
use servitor::memory::{HybridStore, MemoryStore, SqliteStore};
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

    let mode = config.deployment.mode;
    tracing::info!(
        id = %identity.public_id(),
        mode = ?mode,
        "starting daemon mode"
    );

    // Initialize components
    let provider = create_provider(&config.llm)?;
    let mut mcp_pool = McpPool::from_config(config)?;
    mcp_pool.initialize_all().await?;

    let mut scope_enforcer = ScopeEnforcer::new();
    for (name, mcp_config) in &config.mcp {
        scope_enforcer.add_policy(name, &mcp_config.scope)?;
    }

    let egregore = EgregoreClient::new(&config.egregore.api_url);

    // Initialize memory store for personal/hybrid modes
    let memory_store: Option<Arc<dyn MemoryStore>> = match mode {
        DeploymentMode::Personal | DeploymentMode::Hybrid => {
            let db_path = PathBuf::from(&config.memory.database);
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let sqlite = SqliteStore::open(&db_path)?;
            tracing::info!(path = %db_path.display(), "opened memory database");

            let store = match mode {
                DeploymentMode::Hybrid => {
                    // Create a separate egregore client for the hybrid store
                    let hybrid_egregore = EgregoreClient::new(&config.egregore.api_url);
                    HybridStore::new(sqlite, Some(hybrid_egregore), config.memory.cache_ttl_hours)
                }
                _ => HybridStore::local_only(sqlite, config.memory.cache_ttl_hours),
            };

            Some(Arc::new(store))
        }
        DeploymentMode::Server => None,
    };

    // Build event router for non-comms sources
    let mut event_router = EventRouter::new();

    // Add local source for personal/hybrid modes
    if let Some(ref store) = memory_store {
        let local_source = LocalSource::new(store.clone());
        event_router.add_source(Box::new(local_source));
        tracing::info!("local task queue enabled");
    }

    // Add cron source if we have scheduled tasks
    if !config.schedule.is_empty() {
        let cron_source = CronSource::new(&config.schedule)?;
        event_router.add_source(Box::new(cron_source));
        tracing::info!(tasks = config.schedule.len(), "cron source enabled");
    }

    // Add SSE source if subscribe is enabled (server or hybrid mode)
    if config.egregore.subscribe && mode != DeploymentMode::Personal {
        let capabilities = mcp_pool.capabilities();
        let sse_source = SseSource::new(&config.egregore.api_url, capabilities);
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

    // Publish initial profile (skip in personal mode)
    if mode != DeploymentMode::Personal {
        let profile = build_profile(&identity, &mcp_pool, config);
        if let Err(e) = egregore.publish_profile(&profile).await {
            tracing::warn!(error = %e, "failed to publish profile (egregore may be offline)");
        }
    }

    // Main event loop
    let heartbeat_interval = std::time::Duration::from_secs(config.heartbeat.interval_secs);
    let sync_interval = std::time::Duration::from_secs(config.sync.interval_secs);
    let poll_interval = std::time::Duration::from_millis(100);
    let mut last_heartbeat = std::time::Instant::now();
    let mut last_sync = std::time::Instant::now();

    tracing::info!(
        sources = event_router.source_count(),
        discord = discord_transport.is_some(),
        mode = ?mode,
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

                // Check heartbeat (skip in personal mode)
                if mode != DeploymentMode::Personal && last_heartbeat.elapsed() >= heartbeat_interval {
                    let profile = build_profile(&identity, &mcp_pool, config);
                    if let Err(e) = egregore.publish_profile(&profile).await {
                        tracing::debug!(error = %e, "heartbeat failed");
                    } else {
                        tracing::debug!("heartbeat published");
                    }
                    last_heartbeat = std::time::Instant::now();
                }

                // Sync pending results (hybrid mode only)
                if mode == DeploymentMode::Hybrid
                    && config.sync.enabled
                    && last_sync.elapsed() >= sync_interval
                {
                    if let Some(ref store) = memory_store {
                        // Downcast to HybridStore for sync
                        if let Some(hybrid) = store
                            .as_ref()
                            .as_any()
                            .and_then(|any| any.downcast_ref::<HybridStore>())
                        {
                            match hybrid.sync_results(config.sync.batch_size).await {
                                Ok(0) => {}
                                Ok(n) => tracing::debug!(synced = n, "synced results to egregore"),
                                Err(e) => tracing::warn!(error = %e, "sync failed"),
                            }
                        }
                    }
                    last_sync = std::time::Instant::now();
                }

                // Clear expired cache periodically
                if mode != DeploymentMode::Server && last_heartbeat.elapsed() < std::time::Duration::from_millis(200) {
                    // Piggyback on heartbeat timing to occasionally clear cache
                    if let Some(ref store) = memory_store {
                        let _ = store.clear_expired_cache().await;
                    }
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

    // Show deployment mode
    let mode_str = match config.deployment.mode {
        DeploymentMode::Server => "SERVER (egregore-connected)",
        DeploymentMode::Personal => "PERSONAL (local-first, offline-capable)",
        DeploymentMode::Hybrid => "HYBRID (local storage + egregore sync)",
    };
    println!("Deployment Mode: {}", mode_str);
    if config.deployment.mode != DeploymentMode::Server {
        println!("  Memory DB: {}", config.memory.database);
        if config.deployment.mode == DeploymentMode::Hybrid {
            println!("  Sync interval: {}s", config.sync.interval_secs);
            println!("  Sync enabled: {}", config.sync.enabled);
        }
    }
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

[deployment]
mode = "server"

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

[memory]
database = "~/.servitor/memory.db"
cache_ttl_hours = 168

[sync]
interval_secs = 60
batch_size = 10
enabled = true
"#;
    Config::parse(toml)
}

/// Simple hash for task ID generation in exec mode.
fn md5_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
