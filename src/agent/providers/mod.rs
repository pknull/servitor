//! LLM providers -- re-exported from thallus-core, plus Codex.

pub use thallus_core::provider::*;

mod codex;
pub use codex::CodexOAuthProvider;

use crate::config::LlmConfig;
use crate::error::{Result, ServitorError};

/// Create a provider from configuration (servitor version, includes Codex).
///
/// Shadows thallus-core's `create_provider` to add the codex provider.
pub fn create_provider(config: &LlmConfig) -> Result<Box<dyn Provider>> {
    let core_config = thallus_core::config::LlmConfig {
        provider: config.provider.clone(),
        model: config.model.clone(),
        api_key_env: config.api_key_env.clone(),
        base_url: config.base_url.clone(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        max_retries: None,
        initial_backoff_ms: None,
        max_backoff_ms: None,
    };

    match config.provider.as_str() {
        "codex" => Ok(Box::new(CodexOAuthProvider::new(config)?)),
        _ => thallus_core::provider::create_provider(&core_config).map_err(ServitorError::from),
    }
}
