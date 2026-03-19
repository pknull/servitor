//! Runtime context for CLI command execution.
//!
//! Consolidates shared initialization logic across CLI commands.

use std::path::PathBuf;

use crate::a2a::A2aPool;
use crate::agent::create_provider;
use crate::agent::provider::Provider;
use crate::authority::{load_runtime_authority, Authority};
use crate::config::Config;
use crate::egregore::EgregoreClient;
use crate::error::Result;
use crate::identity::Identity;
use crate::mcp::McpPool;
use crate::scope::ScopeEnforcer;

/// Shared runtime context for task execution.
///
/// Encapsulates the common components needed by all CLI commands:
/// identity, authority, MCP pool, A2A pool, scope enforcer, and LLM provider.
///
/// Provider is optional to support worker/coordinator modes that don't require LLM.
pub struct RuntimeContext {
    pub identity: Identity,
    pub authority: Authority,
    pub mcp_pool: McpPool,
    pub a2a_pool: A2aPool,
    pub scope_enforcer: ScopeEnforcer,
    pub provider: Option<Box<dyn Provider>>,
    pub egregore: EgregoreClient,
}

impl RuntimeContext {
    /// Create a new runtime context from configuration.
    ///
    /// This performs all the shared initialization:
    /// - Load or generate identity
    /// - Load authority (or open mode if insecure)
    /// - Create LLM provider
    /// - Initialize MCP pool
    /// - Initialize A2A pool
    /// - Configure scope policies
    pub async fn new(config: &Config, insecure: bool) -> Result<Self> {
        let identity_dir = PathBuf::from(&config.identity.data_dir);
        let identity = Identity::load_or_generate(&identity_dir)?;
        let authority = load_runtime_authority(&identity_dir, insecure)?;

        let provider = match &config.llm {
            Some(llm_config) => Some(create_provider(llm_config)?),
            None => None,
        };
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

        Ok(Self {
            identity,
            authority,
            mcp_pool,
            a2a_pool,
            scope_enforcer,
            provider,
            egregore,
        })
    }

    /// Shutdown all MCP servers cleanly.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.mcp_pool.shutdown_all().await
    }

    /// Get the LLM provider, returning an error if not configured.
    ///
    /// Use this for commands that require LLM reasoning (exec, hook, daemon with SSE).
    pub fn require_provider(&self) -> Result<&dyn Provider> {
        self.provider.as_ref().map(|p| p.as_ref()).ok_or_else(|| {
            crate::error::ServitorError::Config {
                reason:
                    "LLM provider not configured. Add [llm] section to config for reasoning modes."
                        .into(),
            }
        })
    }

    /// Check if LLM provider is available.
    pub fn has_provider(&self) -> bool {
        self.provider.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn context_creation_fails_without_authority() {
        let config = Config::from_str(
            r#"
[llm]
provider = "ollama"
model = "test"

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
        let dir = tempfile::tempdir().unwrap();
        let config = Config::from_str(&format!(
            r#"
[llm]
provider = "ollama"
model = "test"

[identity]
data_dir = "{}"
"#,
            dir.path().display()
        ))
        .unwrap();

        let result = RuntimeContext::new(&config, true).await;
        assert!(result.is_ok());
    }
}
