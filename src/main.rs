//! Servitor — Egregore network task executor.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use servitor::cli::{run_daemon, run_exec, run_hook, run_info, run_init};
use servitor::config::Config;
use servitor::error::Result;

#[derive(Parser)]
#[command(name = "servitor")]
#[command(about = "Egregore network task executor using MCP servers")]
#[command(version)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "servitor.toml")]
    config: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "warn")]
    log_level: String,

    /// Disable authority checks. Development-only.
    #[arg(long, global = true)]
    insecure: bool,

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
        /// Plan the intended tool calls, validate them, and do not execute anything
        #[arg(long, conflicts_with = "plan_first")]
        dry_run: bool,

        /// Plan and validate tool calls before starting execution
        #[arg(long)]
        plan_first: bool,

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
        Config::minimal_defaults()?
    } else {
        return Err(servitor::ServitorError::Config {
            reason: format!("config file not found: {}", cli.config.display()),
        });
    };

    config.expand_paths();

    match cli.command {
        Some(Commands::Run { hook }) => {
            if hook {
                run_hook(&config, cli.insecure).await
            } else {
                run_daemon(&config, cli.insecure).await
            }
        }
        Some(Commands::Exec {
            dry_run,
            plan_first,
            prompt,
        }) => run_exec(&config, &prompt, cli.insecure, dry_run, plan_first).await,
        Some(Commands::Info) => run_info(&config, cli.insecure).await,
        Some(Commands::Init { force }) => run_init(&config, force).await,
        None => {
            // Default to daemon mode
            run_daemon(&config, cli.insecure).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use servitor::a2a::A2aPool;
    use servitor::authority::{authorize_local_exec, load_runtime_authority, Authority};
    use servitor::egregore::{build_profile, ServitorLoad, ServitorStats};
    use servitor::identity::Identity;
    use servitor::mcp::McpPool;
    use servitor::runtime::RuntimeStats;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn missing_authority_requires_explicit_insecure() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_runtime_authority(dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("authority file not found"));
    }

    #[test]
    fn insecure_flag_restores_open_mode() {
        let dir = tempfile::tempdir().unwrap();
        let authority = load_runtime_authority(dir.path(), true).unwrap();
        assert!(authority.is_open_mode());
    }

    #[test]
    fn existing_authority_loads_in_restricted_mode() {
        let dir = tempfile::tempdir().unwrap();
        let authority_path = dir.path().join("authority.toml");
        fs::write(
            &authority_path,
            r#"
[[keeper]]
name = "dev"
egregore = "@dev.ed25519"

[[permission]]
keeper = "dev"
place = "*"
skills = ["*"]
"#,
        )
        .unwrap();

        let authority = load_runtime_authority(dir.path(), false).unwrap();
        assert!(!authority.is_open_mode());
    }

    #[test]
    fn local_exec_authorizes_as_servitor_identity() {
        let authority = Authority::from_config(
            servitor::authority::AuthorityConfig::from_toml(
                r#"
[[keeper]]
name = "servitor"
egregore = "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519"

[[permission]]
keeper = "servitor"
place = "*"
skills = ["*"]
"#,
            )
            .unwrap(),
        );
        let identity = Identity::generate();
        let matching = Authority::from_config(servitor::authority::AuthorityConfig {
            keepers: vec![servitor::authority::Keeper {
                name: "servitor".to_string(),
                egregore: Some(identity.public_id().0.clone()),
                discord: None,
                http_token: None,
            }],
            permissions: authority
                .permissions_for("servitor")
                .into_iter()
                .cloned()
                .collect(),
        });

        let keeper = authorize_local_exec(&matching, &identity).unwrap();
        assert_eq!(keeper.as_deref(), Some("servitor"));
    }

    #[test]
    fn local_exec_denies_unknown_servitor_identity() {
        let authority = Authority::from_config(
            servitor::authority::AuthorityConfig::from_toml(
                r#"
[[keeper]]
name = "other"
egregore = "@BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=.ed25519"

[[permission]]
keeper = "other"
place = "*"
skills = ["*"]
"#,
            )
            .unwrap(),
        );
        let identity = Identity::generate();

        let err = authorize_local_exec(&authority, &identity).unwrap_err();
        assert!(matches!(err, servitor::ServitorError::Unauthorized { .. }));
    }
    #[test]
    fn exec_dry_run_flag_parses() {
        let cli = Cli::try_parse_from(["servitor", "exec", "--dry-run", "inspect files"]).unwrap();
        match cli.command {
            Some(Commands::Exec {
                dry_run,
                plan_first,
                prompt,
            }) => {
                assert!(dry_run);
                assert!(!plan_first);
                assert_eq!(prompt, "inspect files");
            }
            _ => panic!("expected exec command"),
        }
    }

    #[test]
    fn exec_plan_first_flag_parses() {
        let cli =
            Cli::try_parse_from(["servitor", "exec", "--plan-first", "inspect files"]).unwrap();
        match cli.command {
            Some(Commands::Exec {
                dry_run,
                plan_first,
                prompt,
            }) => {
                assert!(!dry_run);
                assert!(plan_first);
                assert_eq!(prompt, "inspect files");
            }
            _ => panic!("expected exec command"),
        }
    }

    #[test]
    fn exec_plan_flags_conflict() {
        let result = Cli::try_parse_from([
            "servitor",
            "exec",
            "--dry-run",
            "--plan-first",
            "inspect files",
        ]);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn build_profile_includes_runtime_monitoring_data() {
        let config = Config::from_str(
            r#"
[llm]
provider = "ollama"
model = "llama3.3:70b"

[heartbeat]
interval_secs = 15
include_runtime_monitoring = true

[mcp.shell]
transport = "stdio"
command = "nonexistent-mcp-server"
scope.allow = ["execute:/tmp/*"]
"#,
        )
        .unwrap();
        let identity = Identity::generate();
        let mcp_pool = McpPool::from_config(&config).unwrap();
        let a2a_pool = A2aPool::new();
        let mut runtime_stats = RuntimeStats::new();
        runtime_stats.set_started_at(Instant::now() - Duration::from_secs(42));
        runtime_stats.record_task_offer();
        runtime_stats.start_task();
        runtime_stats.finish_task(true, Some("test"));

        let profile = build_profile(&identity, &mcp_pool, &a2a_pool, &config, &runtime_stats).await;
        assert_eq!(profile.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(profile.heartbeat_interval_ms, 15000);
        assert_eq!(profile.uptime_secs, 42);
        assert_eq!(profile.load.tasks_executing, 0);
        assert_eq!(profile.load.tasks_queued, 0);
        assert_eq!(profile.stats.tasks_offered, 1);
        assert_eq!(profile.stats.tasks_executed, 1);
        assert_eq!(profile.stats.tasks_failed, 0);
        assert!(profile.last_task_ts.is_some());
        assert_eq!(profile.mcp_servers.len(), 1);
        assert_eq!(profile.mcp_servers[0].name, "shell");
        assert_eq!(profile.mcp_servers[0].transport, "stdio");
        assert_eq!(profile.scopes["shell"].allow, vec!["execute:/tmp/*"]);
        assert_eq!(
            profile.mcp_servers[0].status,
            servitor::egregore::McpServerHealth::Unavailable
        );
    }

    #[tokio::test]
    async fn build_profile_omits_runtime_monitoring_data_by_default() {
        let config = Config::from_str(
            r#"
[llm]
provider = "ollama"
model = "llama3.3:70b"

[heartbeat]
interval_secs = 15

[mcp.shell]
transport = "stdio"
command = "nonexistent-mcp-server"
scope.allow = ["execute:/tmp/*"]
"#,
        )
        .unwrap();
        let identity = Identity::generate();
        let mcp_pool = McpPool::from_config(&config).unwrap();
        let a2a_pool = A2aPool::new();
        let runtime_stats = RuntimeStats::new();

        let profile = build_profile(&identity, &mcp_pool, &a2a_pool, &config, &runtime_stats).await;
        assert_eq!(profile.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(profile.uptime_secs, 0);
        assert!(profile.mcp_servers.is_empty());
        assert_eq!(profile.load, ServitorLoad::default());
        assert_eq!(profile.stats, ServitorStats::default());
        assert!(profile.last_task_ts.is_none());
    }
}
