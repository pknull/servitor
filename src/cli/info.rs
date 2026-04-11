//! Info command implementation.

use std::path::PathBuf;

use crate::authority::Authority;
use crate::config::Config;
use crate::error::Result;
use crate::identity::Identity;

/// Show identity and capabilities.
///
/// Displays the servitor identity, authority configuration, MCP servers,
/// and scheduled tasks.
pub async fn run_info(config: &Config, insecure: bool) -> Result<()> {
    let identity_dir = PathBuf::from(&config.identity.data_dir);
    let identity = Identity::load_or_generate(&identity_dir)?;

    let authority_path = identity_dir.join("authority.toml");
    let authority = if authority_path.exists() {
        Some(Authority::load(&authority_path)?)
    } else {
        None
    };

    println!("Identity: {}", identity.public_id());
    println!("Data dir: {}", config.identity.data_dir);
    println!();

    // Show authority status
    if insecure {
        println!("Authority: INSECURE OPEN MODE");
        println!("  --insecure disables keeper authorization and is development-only.");
    } else if let Some(authority) = authority {
        if authority.is_open_mode() {
            println!("Authority: INSECURE OPEN MODE");
            println!("  Explicit insecure override is active.");
        } else {
            println!("Authority: RESTRICTED");
            println!("  File: {}", authority_path.display());
        }
    } else {
        println!("Authority: BLOCKED");
        println!("  No authority.toml found at {}", authority_path.display());
        println!(
            "  Copy authority.example.toml into place, or use --insecure for development only."
        );
    }
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
        if !mcp.on_notification.is_empty() {
            let tools: Vec<_> = mcp
                .on_notification
                .iter()
                .map(|call| call.name.as_str())
                .collect();
            println!("    on_notification tools: {:?}", tools);
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
            if let Some(ref prompt) = task.prompt {
                println!("    prompt: {}", prompt);
            }
            let tools: Vec<_> = task
                .tool_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect();
            println!("    tool_calls: {:?}", tools);
            if task.publish {
                println!("    publish: true");
            }
        }
    }

    if !config.watch.is_empty() {
        println!();
        println!("Watchers:");
        for watch in &config.watch {
            println!("  - {} ({}::{})", watch.name, watch.mcp, watch.event);
            if let Some(ref prompt) = watch.prompt {
                println!("    prompt: {}", prompt);
            }
            let tools: Vec<_> = watch
                .tool_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect();
            println!("    tool_calls: {:?}", tools);
        }
    }

    Ok(())
}
