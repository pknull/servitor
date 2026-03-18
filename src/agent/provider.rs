//! LLM Provider trait and implementations.
//!
//! All providers are compiled in; runtime selection via config.

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;
use crate::error::{Result, ServitorError};
use crate::mcp::LlmTool;

/// Provider capabilities.
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_streaming: bool,
    pub max_tokens: Option<u32>,
}

/// Message role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// Message content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

/// Conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    pub fn tool_results(results: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: results,
        }
    }
}

/// Stop reason from LLM response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EndTurn => "end_turn",
            Self::ToolUse => "tool_use",
            Self::MaxTokens => "max_tokens",
            Self::StopSequence => "stop_sequence",
        }
    }
}

/// Chat response from provider.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

impl ChatResponse {
    /// Extract tool_use blocks from the response.
    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    Some((id.as_str(), name.as_str(), input))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get text content from the response.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Token usage statistics.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// LLM Provider trait.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get provider name for metrics.
    fn name(&self) -> &str;

    /// Get provider capabilities.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Send a chat completion request.
    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[LlmTool],
    ) -> Result<ChatResponse>;
}

/// Create a provider from configuration.
pub fn create_provider(config: &LlmConfig) -> Result<Box<dyn Provider>> {
    match config.provider.as_str() {
        "anthropic" => Ok(Box::new(AnthropicProvider::new(config)?)),
        "openai" | "ollama" | "openai-compat" => Ok(Box::new(OpenAiCompatProvider::new(config)?)),
        "codex" => Ok(Box::new(CodexOAuthProvider::new(config)?)),
        "claude-code" => Ok(Box::new(ClaudeCodeProvider::new(config)?)),
        other => Err(ServitorError::Provider {
            reason: format!("unknown provider: {}", other),
        }),
    }
}

/// Anthropic Claude provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let api_key_env = config
            .api_key_env
            .as_ref()
            .ok_or_else(|| ServitorError::Config {
                reason: "anthropic provider requires api_key_env".into(),
            })?;

        let api_key = std::env::var(api_key_env).map_err(|_| ServitorError::Config {
            reason: format!("environment variable {} not set", api_key_env),
        })?;

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model: config.model.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
        })
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
            supports_vision: true,
            supports_streaming: true,
            max_tokens: Some(self.max_tokens),
        }
    }

    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[LlmTool],
    ) -> Result<ChatResponse> {
        let url = "https://api.anthropic.com/v1/messages";

        // Build request body
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": messages,
        });

        if !tools.is_empty() {
            // Convert tools to Anthropic format
            let anthropic_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(anthropic_tools);
        }

        let response = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ServitorError::Provider {
                reason: format!("request failed: {}", e),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Provider {
                reason: format!("API error {}: {}", status, body),
            });
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| ServitorError::Provider {
                reason: format!("failed to parse response: {}", e),
            })?;

        // Parse response
        let content = parse_anthropic_content(&response_body)?;
        let stop_reason = match response_body["stop_reason"].as_str() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        };

        let usage = Usage {
            input_tokens: response_body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: response_body["usage"]["output_tokens"]
                .as_u64()
                .unwrap_or(0) as u32,
        };

        Ok(ChatResponse {
            content,
            stop_reason,
            usage,
        })
    }
}

fn parse_anthropic_content(response: &serde_json::Value) -> Result<Vec<ContentBlock>> {
    let content_array = response["content"]
        .as_array()
        .ok_or_else(|| ServitorError::Provider {
            reason: "response missing content array".into(),
        })?;

    let mut blocks = Vec::new();
    for item in content_array {
        match item["type"].as_str() {
            Some("text") => {
                if let Some(text) = item["text"].as_str() {
                    blocks.push(ContentBlock::text(text));
                }
            }
            Some("tool_use") => {
                let id = item["id"].as_str().unwrap_or("").to_string();
                let name = item["name"].as_str().unwrap_or("").to_string();
                let input = item["input"].clone();
                blocks.push(ContentBlock::tool_use(id, name, input));
            }
            _ => {}
        }
    }

    Ok(blocks)
}

/// OpenAI-compatible provider (works with OpenAI, Ollama, vLLM, etc.)
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    max_tokens: u32,
}

impl OpenAiCompatProvider {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let base_url = match config.provider.as_str() {
            "ollama" => config
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string()),
            "openai" => "https://api.openai.com/v1".to_string(),
            "openai-compat" => config
                .base_url
                .clone()
                .ok_or_else(|| ServitorError::Config {
                    reason: "openai-compat requires base_url".into(),
                })?,
            _ => {
                return Err(ServitorError::Config {
                    reason: format!("unsupported provider: {}", config.provider),
                })
            }
        };

        let api_key = config
            .api_key_env
            .as_ref()
            .and_then(|env| std::env::var(env).ok());

        Ok(Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: config.model.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
        })
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        "openai-compat"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
            supports_vision: false,
            supports_streaming: true,
            max_tokens: Some(self.max_tokens),
        }
    }

    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[LlmTool],
    ) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        // Convert messages to OpenAI format
        let mut openai_messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system
        })];

        for msg in messages {
            let content = convert_content_to_openai(&msg.content);
            openai_messages.push(serde_json::json!({
                "role": match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => "system",
                },
                "content": content,
            }));
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": openai_messages,
        });

        if !tools.is_empty() {
            let openai_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(openai_tools);
        }

        let mut request = self.client.post(&url).json(&body);

        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.send().await.map_err(|e| ServitorError::Provider {
            reason: format!("request failed: {}", e),
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Provider {
                reason: format!("API error {}: {}", status, body),
            });
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| ServitorError::Provider {
                reason: format!("failed to parse response: {}", e),
            })?;

        // Parse OpenAI response
        let choice = &response_body["choices"][0];
        let message = &choice["message"];

        let mut content = Vec::new();

        // Text content
        if let Some(text) = message["content"].as_str() {
            if !text.is_empty() {
                content.push(ContentBlock::text(text));
            }
        }

        // Tool calls
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for call in tool_calls {
                let id = call["id"].as_str().unwrap_or("").to_string();
                let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                let arguments: serde_json::Value = call["function"]["arguments"]
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::json!({}));
                content.push(ContentBlock::tool_use(id, name, arguments));
            }
        }

        let stop_reason = match choice["finish_reason"].as_str() {
            Some("stop") => StopReason::EndTurn,
            Some("tool_calls") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let usage = Usage {
            input_tokens: response_body["usage"]["prompt_tokens"]
                .as_u64()
                .unwrap_or(0) as u32,
            output_tokens: response_body["usage"]["completion_tokens"]
                .as_u64()
                .unwrap_or(0) as u32,
        };

        Ok(ChatResponse {
            content,
            stop_reason,
            usage,
        })
    }
}

fn convert_content_to_openai(content: &[ContentBlock]) -> serde_json::Value {
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();

    if texts.len() == 1 {
        serde_json::Value::String(texts[0].to_string())
    } else {
        serde_json::Value::String(texts.join("\n"))
    }
}

/// Codex OAuth provider — reads tokens from OpenClaw's auth-profiles.json format.
pub struct CodexOAuthProvider {
    client: reqwest::Client,
    token_file: std::path::PathBuf,
    profile_name: String,
    model: String,
    max_tokens: u32,
    cached_token: std::sync::RwLock<Option<CachedToken>>,
}

#[derive(Clone)]
struct CachedToken {
    access: String,
    refresh: String,
    expires: i64,
    account_id: Option<String>,
}

impl CodexOAuthProvider {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let token_file = config
            .token_file
            .as_ref()
            .map(|p| shellexpand::tilde(p).to_string())
            .ok_or_else(|| ServitorError::Config {
                reason: "codex provider requires token_file".into(),
            })?;

        let profile_name = config
            .oauth_profile
            .clone()
            .unwrap_or_else(|| "openai-codex:default".to_string());

        Ok(Self {
            client: reqwest::Client::new(),
            token_file: std::path::PathBuf::from(token_file),
            profile_name,
            model: config.model.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
            cached_token: std::sync::RwLock::new(None),
        })
    }

    fn load_token(&self) -> Result<CachedToken> {
        // Check cache first
        if let Ok(guard) = self.cached_token.read() {
            if let Some(ref cached) = *guard {
                // Token valid for at least 5 more minutes
                let now_ms = chrono::Utc::now().timestamp_millis();
                if cached.expires > now_ms + 300_000 {
                    return Ok(cached.clone());
                }
            }
        }

        // Load from file
        let content =
            std::fs::read_to_string(&self.token_file).map_err(|e| ServitorError::Config {
                reason: format!("failed to read token file: {}", e),
            })?;

        let json: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| ServitorError::Config {
                reason: format!("failed to parse token file: {}", e),
            })?;

        // Support two formats:
        // 1. Codex CLI auth.json format: { "tokens": { "access_token": "...", "refresh_token": "..." } }
        // 2. OpenClaw auth-profiles.json format: { "profiles": { "name": { "access": "...", "refresh": "..." } } }

        let (access, refresh, expires, account_id) = if let Some(tokens) = json.get("tokens") {
            // Codex CLI format
            let access = tokens
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServitorError::Config {
                    reason: "missing access_token in tokens".into(),
                })?
                .to_string();

            let refresh = tokens
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServitorError::Config {
                    reason: "missing refresh_token in tokens".into(),
                })?
                .to_string();

            // Parse exp from JWT to get expiration (access_token is a JWT)
            let expires = self.parse_jwt_exp(&access).unwrap_or(0);

            // Get account_id from root level
            let account_id = tokens
                .get("account_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            (access, refresh, expires, account_id)
        } else if let Some(profiles) = json.get("profiles") {
            // OpenClaw auth-profiles.json format
            let profile =
                profiles
                    .get(&self.profile_name)
                    .ok_or_else(|| ServitorError::Config {
                        reason: format!("profile '{}' not found in token file", self.profile_name),
                    })?;

            let access = profile
                .get("access")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServitorError::Config {
                    reason: "missing access token".into(),
                })?
                .to_string();

            let refresh = profile
                .get("refresh")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServitorError::Config {
                    reason: "missing refresh token".into(),
                })?
                .to_string();

            let expires = profile.get("expires").and_then(|v| v.as_i64()).unwrap_or(0);

            let account_id = profile
                .get("accountId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            (access, refresh, expires, account_id)
        } else {
            return Err(ServitorError::Config {
                reason: "token file must have 'tokens' or 'profiles' field".into(),
            });
        };

        let token = CachedToken {
            access,
            refresh,
            expires,
            account_id,
        };

        // Cache it
        if let Ok(mut guard) = self.cached_token.write() {
            *guard = Some(token.clone());
        }

        Ok(token)
    }

    /// Parse JWT exp claim to get expiration time in milliseconds
    fn parse_jwt_exp(&self, token: &str) -> Option<i64> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        // Decode base64 payload (middle part)
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
        // exp is in seconds, convert to milliseconds
        claims.get("exp")?.as_i64().map(|exp| exp * 1000)
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<CachedToken> {
        tracing::info!("refreshing Codex OAuth token");

        let response = self
            .client
            .post("https://auth.openai.com/oauth/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"), // Codex CLI client ID
            ])
            .send()
            .await
            .map_err(|e| ServitorError::Provider {
                reason: format!("token refresh request failed: {}", e),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Provider {
                reason: format!("token refresh failed {}: {}", status, body),
            });
        }

        let body: serde_json::Value =
            response.json().await.map_err(|e| ServitorError::Provider {
                reason: format!("failed to parse refresh response: {}", e),
            })?;

        let access = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServitorError::Provider {
                reason: "missing access_token in refresh response".into(),
            })?
            .to_string();

        let new_refresh = body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| refresh_token.to_string());

        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);

        let expires = chrono::Utc::now().timestamp_millis() + (expires_in * 1000);

        // Preserve account_id from cached token or reload from file
        let account_id = self
            .cached_token
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|t| t.account_id.clone()))
            .or_else(|| self.load_account_id());

        let token = CachedToken {
            access: access.clone(),
            refresh: new_refresh.clone(),
            expires,
            account_id,
        };

        // Update cache
        if let Ok(mut guard) = self.cached_token.write() {
            *guard = Some(token.clone());
        }

        // Save back to file
        self.save_token(&access, &new_refresh, expires)?;

        Ok(token)
    }

    fn load_account_id(&self) -> Option<String> {
        let content = std::fs::read_to_string(&self.token_file).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;

        // Try Codex format first
        if let Some(tokens) = json.get("tokens") {
            return tokens
                .get("account_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        // Try OpenClaw format
        if let Some(profiles) = json.get("profiles") {
            if let Some(profile) = profiles.get(&self.profile_name) {
                return profile
                    .get("accountId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }

        None
    }

    fn save_token(&self, access: &str, refresh: &str, expires: i64) -> Result<()> {
        let content =
            std::fs::read_to_string(&self.token_file).map_err(|e| ServitorError::Config {
                reason: format!("failed to read token file for update: {}", e),
            })?;

        let mut profiles: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| ServitorError::Config {
                reason: format!("failed to parse token file for update: {}", e),
            })?;

        if let Some(profile) = profiles
            .get_mut("profiles")
            .and_then(|p| p.get_mut(&self.profile_name))
        {
            profile["access"] = serde_json::Value::String(access.to_string());
            profile["refresh"] = serde_json::Value::String(refresh.to_string());
            profile["expires"] = serde_json::Value::Number(expires.into());
        }

        let content =
            serde_json::to_string_pretty(&profiles).map_err(|e| ServitorError::Config {
                reason: format!("failed to serialize token file: {}", e),
            })?;

        std::fs::write(&self.token_file, content).map_err(|e| ServitorError::Config {
            reason: format!("failed to write token file: {}", e),
        })?;

        tracing::info!("updated token file");
        Ok(())
    }

    async fn get_valid_token(&self) -> Result<CachedToken> {
        let token = self.load_token()?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        if token.expires <= now_ms + 60_000 {
            // Token expired or expiring within 1 minute, refresh it
            self.refresh_token(&token.refresh).await
        } else {
            Ok(token)
        }
    }
}

#[async_trait]
impl Provider for CodexOAuthProvider {
    fn name(&self) -> &str {
        "codex"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
            supports_vision: false,
            supports_streaming: true,
            max_tokens: Some(self.max_tokens),
        }
    }

    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[LlmTool],
    ) -> Result<ChatResponse> {
        let token = self.get_valid_token().await?;
        // Use ChatGPT backend API for ChatGPT OAuth tokens
        // (api.openai.com requires api.responses.write scope which ChatGPT tokens don't have)
        let url = "https://chatgpt.com/backend-api/codex/responses";
        let access_token = &token.access;

        // Convert messages to Responses API input format
        let mut input: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "developer", // Responses API uses "developer" for system
            };

            // Build content items for this message
            let mut content_items: Vec<serde_json::Value> = Vec::new();

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        content_items.push(serde_json::json!({
                            "type": "input_text",
                            "text": text
                        }));
                    }
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input: args,
                    } => {
                        // Tool use from assistant is a function_call in Responses API
                        input.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": serde_json::to_string(args).unwrap_or_default()
                        }));
                        continue; // Don't add to content_items
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // Tool results are function_call_output items
                        input.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": if *is_error {
                                format!("Error: {}", content)
                            } else {
                                content.clone()
                            }
                        }));
                        continue; // Don't add to content_items
                    }
                }
            }

            // If we have content items, add as a message
            if !content_items.is_empty() {
                input.push(serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": content_items
                }));
            }
        }

        // Build tools in Responses API format
        let responses_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "instructions": system,
            "input": input,
            "store": false,
            "stream": true,  // ChatGPT backend requires streaming
        });

        if !responses_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(responses_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        tracing::debug!(url = %url, "sending Responses API streaming request");

        // Build request
        let mut request = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        // Add ChatGPT-Account-ID header if available and non-empty
        if let Some(ref account_id) = token.account_id {
            if !account_id.is_empty() {
                request = request.header("ChatGPT-Account-ID", account_id);
            }
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| ServitorError::Provider {
                reason: format!("request failed: {}", e),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ServitorError::Provider {
                reason: format!("API error {}: {}", status, body),
            });
        }

        // Parse SSE stream manually (ChatGPT API doesn't set Content-Type header)
        use eventsource_stream::Eventsource;
        let stream = response.bytes_stream();
        let mut stream = Eventsource::eventsource(stream);

        // Collect SSE events until response.completed
        let mut content = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();
        let mut accumulated_text = String::new();
        let mut function_calls: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();

        while let Some(event) = stream.next().await {
            match event {
                Ok(evt) => {
                    tracing::debug!(event = %evt.event, data_len = evt.data.len(), "SSE event received");

                    if evt.event == "error" {
                        return Err(ServitorError::Provider {
                            reason: format!("SSE error: {}", evt.data),
                        });
                    }

                    // Skip empty data
                    if evt.data.is_empty() || evt.data == "[DONE]" {
                        continue;
                    }

                    // Parse the event data
                    let data: serde_json::Value = match serde_json::from_str(&evt.data) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::warn!(error = %e, raw = %evt.data, "Failed to parse SSE data");
                            continue;
                        }
                    };

                    // ChatGPT backend may use different event format
                    // Check for direct "response" field first (simple response)
                    if let Some(response_text) = data.get("response").and_then(|v| v.as_str()) {
                        tracing::debug!(response_len = response_text.len(), "Got direct response");
                        accumulated_text.push_str(response_text);
                        continue;
                    }

                    match data["type"].as_str() {
                        Some("response.output_text.delta") => {
                            // Accumulate text deltas
                            if let Some(delta) = data["delta"].as_str() {
                                accumulated_text.push_str(delta);
                            }
                        }
                        Some("response.function_call_arguments.delta") => {
                            // Accumulate function call arguments
                            if let Some(call_id) = data["call_id"].as_str() {
                                if let Some(delta) = data["delta"].as_str() {
                                    function_calls
                                        .entry(call_id.to_string())
                                        .or_insert_with(|| {
                                            let name =
                                                data["name"].as_str().unwrap_or("").to_string();
                                            (name, String::new())
                                        })
                                        .1
                                        .push_str(delta);
                                }
                            }
                        }
                        Some("response.output_item.added") => {
                            // Track new function calls
                            if data["item"]["type"].as_str() == Some("function_call") {
                                if let (Some(call_id), Some(name)) = (
                                    data["item"]["call_id"].as_str(),
                                    data["item"]["name"].as_str(),
                                ) {
                                    function_calls
                                        .entry(call_id.to_string())
                                        .or_insert_with(|| (name.to_string(), String::new()));
                                }
                            }
                        }
                        Some("response.completed") => {
                            // Final response - extract usage and status
                            if let Some(response) = data.get("response") {
                                // Get usage
                                if let Some(u) = response.get("usage") {
                                    usage = Usage {
                                        input_tokens: u["input_tokens"].as_u64().unwrap_or(0)
                                            as u32,
                                        output_tokens: u["output_tokens"].as_u64().unwrap_or(0)
                                            as u32,
                                    };
                                }

                                // Check status
                                if let Some("incomplete") = response["status"].as_str() {
                                    if response["incomplete_details"]["reason"].as_str()
                                        == Some("max_output_tokens")
                                    {
                                        stop_reason = StopReason::MaxTokens;
                                    }
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = ?e, "SSE stream error");
                    return Err(ServitorError::Provider {
                        reason: format!("SSE error: {:?}", e),
                    });
                }
            }
        }

        // Build content from accumulated data
        if !accumulated_text.is_empty() {
            content.push(ContentBlock::text(accumulated_text));
        }

        for (call_id, (name, arguments)) in function_calls {
            let args: serde_json::Value =
                serde_json::from_str(&arguments).unwrap_or(serde_json::json!({}));
            content.push(ContentBlock::tool_use(call_id, name, args));
            stop_reason = StopReason::ToolUse;
        }

        Ok(ChatResponse {
            content,
            stop_reason,
            usage,
        })
    }
}

/// Claude Code provider — uses Claude Code CLI via subprocess.
/// No API key needed; authenticates through the installed Claude Code CLI.
pub struct ClaudeCodeProvider {
    #[allow(dead_code)]
    model: String,
    max_tokens: u32,
}

impl ClaudeCodeProvider {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        Ok(Self {
            model: config.model.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
        })
    }
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
            supports_vision: true,
            supports_streaming: true,
            max_tokens: Some(self.max_tokens),
        }
    }

    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[LlmTool],
    ) -> Result<ChatResponse> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;

        // Build prompt from system message and conversation history
        let mut prompt = String::new();

        // Include system context
        if !system.is_empty() {
            prompt.push_str("<system>\n");
            prompt.push_str(system);
            prompt.push_str("\n</system>\n\n");
        }

        // Include available tools in the prompt
        if !tools.is_empty() {
            prompt.push_str("<available_tools>\n");
            for tool in tools {
                let desc = tool.description.as_deref().unwrap_or("No description");
                prompt.push_str(&format!(
                    "- {}: {}\n  Parameters: {}\n",
                    tool.name,
                    desc,
                    serde_json::to_string(&tool.input_schema).unwrap_or_default()
                ));
            }
            prompt.push_str("</available_tools>\n\n");
            prompt.push_str("To use a tool, respond with a JSON block:\n");
            prompt
                .push_str("```tool_use\n{\"name\": \"tool_name\", \"arguments\": {...}}\n```\n\n");
        }

        // Convert message history to text
        for msg in messages {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
            };

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        prompt.push_str(&format!("{}: {}\n", role_label, text));
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        prompt.push_str(&format!(
                            "Assistant used tool {}: {}\n",
                            name,
                            serde_json::to_string(input).unwrap_or_default()
                        ));
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        if *is_error {
                            prompt.push_str(&format!("Tool error: {}\n", content));
                        } else {
                            prompt.push_str(&format!("Tool result: {}\n", content));
                        }
                    }
                }
            }
        }

        tracing::debug!(prompt_len = prompt.len(), "sending to Claude Code CLI");

        // Build command arguments
        // -p/--print enables non-interactive mode
        // --verbose required for stream-json output format
        // Prompt goes at the end as positional argument
        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--max-turns")
            .arg("1")
            .arg(&prompt)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Spawn the process
        let mut child = cmd.spawn().map_err(|e| ServitorError::Provider {
            reason: format!("Failed to spawn Claude CLI: {}", e),
        })?;

        let stdout = child.stdout.take().ok_or_else(|| ServitorError::Provider {
            reason: "Failed to capture stdout".into(),
        })?;

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        // Collect response
        let mut content = Vec::new();
        let mut accumulated_text = String::new();

        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| ServitorError::Provider {
                reason: format!("Failed to read line: {}", e),
            })?
        {
            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON line
            let json: serde_json::Value = match serde_json::from_str(&line) {
                Ok(j) => j,
                Err(e) => {
                    tracing::trace!(error = %e, line = %line, "Failed to parse JSON line");
                    continue;
                }
            };

            // Get message type
            let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

            match msg_type {
                "assistant" => {
                    // Extract content from assistant message
                    if let Some(message) = json.get("message") {
                        if let Some(contents) = message.get("content").and_then(|v| v.as_array()) {
                            for block in contents {
                                let block_type =
                                    block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                match block_type {
                                    "text" => {
                                        if let Some(text) =
                                            block.get("text").and_then(|v| v.as_str())
                                        {
                                            accumulated_text.push_str(text);
                                        }
                                    }
                                    "tool_use" => {
                                        let id = block
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let input = block.get("input").cloned().unwrap_or_default();
                                        content.push(ContentBlock::tool_use(id, name, input));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                "result" => {
                    // Final result - done
                    break;
                }
                "rate_limit_event" => {
                    // Rate limit - log but continue (the CLI will handle waiting)
                    tracing::warn!("Rate limit event received, waiting...");
                }
                _ => {
                    // Other message types (stream_event, etc.) - skip
                    tracing::trace!(msg_type = %msg_type, "Skipping message type");
                }
            }
        }

        // Wait for process to complete
        let status = child.wait().await.map_err(|e| ServitorError::Provider {
            reason: format!("Failed to wait for Claude CLI: {}", e),
        })?;

        if !status.success() {
            return Err(ServitorError::Provider {
                reason: format!("Claude CLI exited with status: {}", status),
            });
        }

        // Parse any tool_use blocks from the accumulated text
        // (Claude Code may emit them inline in markdown)
        if !accumulated_text.is_empty() {
            // Check for ```tool_use blocks in the response
            let tool_uses = parse_tool_use_blocks(&accumulated_text);
            if tool_uses.is_empty() {
                content.push(ContentBlock::text(accumulated_text));
            } else {
                // Extract text before first tool_use
                if let Some(first_idx) = accumulated_text.find("```tool_use") {
                    let before = accumulated_text[..first_idx].trim();
                    if !before.is_empty() {
                        content.push(ContentBlock::text(before));
                    }
                }
                for (name, args) in tool_uses {
                    let id = format!("tool_{}", uuid::Uuid::new_v4());
                    content.push(ContentBlock::tool_use(id, name, args));
                }
            }
        }

        // Determine stop reason
        let stop_reason = if content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
        {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        Ok(ChatResponse {
            content,
            stop_reason,
            usage: Usage::default(), // SDK doesn't expose usage
        })
    }
}

/// Parse tool_use code blocks from markdown response.
fn parse_tool_use_blocks(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut results = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("```tool_use") {
        let after_marker = &remaining[start + 11..];
        if let Some(end) = after_marker.find("```") {
            let json_str = after_marker[..end].trim();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let (Some(name), Some(args)) = (
                    json.get("name").and_then(|v| v.as_str()),
                    json.get("arguments"),
                ) {
                    results.push((name.to_string(), args.clone()));
                }
            }
            remaining = &after_marker[end + 3..];
        } else {
            break;
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_construction() {
        let msg = Message::user("Hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
    }

    #[test]
    fn tool_use_extraction() {
        let response = ChatResponse {
            content: vec![
                ContentBlock::text("Let me check that"),
                ContentBlock::tool_use(
                    "call_1",
                    "shell_execute",
                    serde_json::json!({"command": "ls"}),
                ),
            ],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        };

        let uses = response.tool_uses();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, "shell_execute");
    }
}
