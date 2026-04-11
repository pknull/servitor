//! Runtime context for CLI command execution.
//!
//! Consolidates shared initialization logic across CLI commands.

use std::path::PathBuf;
use std::sync::Arc;

use crate::a2a::A2aPool;
use crate::authority::{load_runtime_authority, Authority};
use crate::config::Config;
use crate::egregore::EgregoreClient;
use crate::error::Result;
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;
use crate::session::SessionStore;

/// Shared runtime context for task execution.
///
/// Encapsulates the common components needed by all CLI commands:
/// identity, authority, MCP pool, A2A pool, scope enforcer, and session store.
///
/// Servitors execute pre-planned tool calls directly — no LLM provider needed.
pub struct RuntimeContext {
    pub identity: Identity,
    pub authority: Authority,
    pub mcp_pool: McpPool,
    pub a2a_pool: A2aPool,
    pub scope_enforcer: ScopeEnforcer,
    pub egregore: EgregoreClient,
    pub session_store: Arc<SessionStore>,
}

impl RuntimeContext {
    /// Create a new runtime context from configuration.
    ///
    /// This performs all the shared initialization:
    /// - Load or generate identity
    /// - Load authority (or open mode if insecure)
    /// - Initialize MCP pool
    /// - Initialize A2A pool
    /// - Configure scope policies
    pub async fn new(config: &Config, insecure: bool) -> Result<Self> {
        let identity_dir = PathBuf::from(&config.identity.data_dir);
        let identity = Identity::load_or_generate(&identity_dir)?;
        let authority = load_runtime_authority(&identity_dir, insecure)?;

        let mut mcp_pool = McpPool::from_config(config)?;
        mcp_pool.initialize_all().await?;

        // Initialize A2A pool (agents are external services, not local processes)
        let mut a2a_pool =
            A2aPool::from_config(config).map_err(|e| crate::error::ServitorError::Config {
                reason: format!("failed to create A2A pool: {}", e),
            })?;
        if !a2a_pool.is_empty() {
            a2a_pool
                .initialize_all()
                .await
                .map_err(|e| crate::error::ServitorError::Config {
                    reason: format!("failed to initialize A2A pool: {}", e),
                })?;
            tracing::info!(
                agents = a2a_pool.agents().len(),
                "initialized A2A agent pool"
            );
        }

        let mut scope_enforcer = ScopeEnforcer::new();
        for (name, mcp_config) in &config.mcp {
            scope_enforcer.add_policy(name, &mcp_config.scope)?;
        }
        // Add A2A scope policies
        for (name, a2a_config) in &config.a2a {
            scope_enforcer.add_policy(name, &a2a_config.scope)?;
        }

        let egregore = EgregoreClient::new(&config.egregore.api_url);

        // Initialize session store
        let session_store = Arc::new(SessionStore::open(&identity_dir)?);
        tracing::debug!("session store: initialized at {}", identity_dir.display());

        Ok(Self {
            identity,
            authority,
            mcp_pool,
            a2a_pool,
            scope_enforcer,
            egregore,
            session_store,
        })
    }

    /// Shutdown all MCP servers cleanly.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.mcp_pool.shutdown_all().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn context_creation_fails_without_authority() {
        let config = Config::from_str(
            r#"
[identity]
data_dir = "/tmp/nonexistent-servitor-test"
"#,
        )
        .unwrap();

        let result = RuntimeContext::new(&config, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn context_creation_succeeds_with_insecure() {
        std::env::set_var("SERVITOR_INSECURE", "1");
        let dir = tempfile::tempdir().unwrap();
        let config = Config::from_str(&format!(
            r#"
[identity]
data_dir = "{}"
"#,
            dir.path().display()
        ))
        .unwrap();

        let result = RuntimeContext::new(&config, true).await;
        assert!(result.is_ok());
        std::env::remove_var("SERVITOR_INSECURE");
    }
}
