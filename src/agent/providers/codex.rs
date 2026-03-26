//! Codex OAuth provider — reads tokens from OpenClaw's auth-profiles.json format.

use async_trait::async_trait;
use futures::StreamExt;

use super::{
    truncate_error_body, ChatResponse, ContentBlock, Message, Provider, ProviderCapabilities, Role,
    StopReason, Usage,
};
use crate::config::LlmConfig;
use crate::error::{Result, ServitorError};
use crate::mcp::LlmTool;

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

        if let Some(tokens) = profiles.get_mut("tokens") {
            // Codex CLI format
            tokens["access_token"] = serde_json::Value::String(access.to_string());
            tokens["refresh_token"] = serde_json::Value::String(refresh.to_string());
            profiles["last_refresh"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
        } else if let Some(profile) = profiles
            .get_mut("profiles")
            .and_then(|p| p.get_mut(&self.profile_name))
        {
            // OpenClaw auth-profiles format
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
                reason: format!("API error {}: {}", status, truncate_error_body(&body)),
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
