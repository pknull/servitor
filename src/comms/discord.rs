//! Discord communication transport.
//!
//! Uses serenity to connect to Discord gateway, receive mentions,
//! and send responses.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serenity::all::{
    ChannelId, Context, CreateMessage, EventHandler, GatewayIntents, Http, Message, MessageId,
    Ready, UserId,
};
use serenity::Client;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info};

use super::{CommsMessage, CommsResponder, CommsResponse, CommsSource, CommsTransport};
use crate::config::DiscordConfig;
use crate::error::{Result, ServitorError};

/// Discord transport implementation.
pub struct DiscordTransport {
    config: DiscordConfig,
    token: String,
    rx: Option<mpsc::Receiver<(CommsMessage, DiscordResponder)>>,
    client_handle: Option<tokio::task::JoinHandle<()>>,
}

impl DiscordTransport {
    pub fn new(config: &DiscordConfig) -> Result<Self> {
        let token = std::env::var(&config.token_env).map_err(|_| ServitorError::Config {
            reason: format!("environment variable {} not set", config.token_env),
        })?;

        Ok(Self {
            config: config.clone(),
            token,
            rx: None,
            client_handle: None,
        })
    }
}

#[async_trait]
impl CommsTransport for DiscordTransport {
    fn name(&self) -> &str {
        "discord"
    }

    async fn connect(&mut self) -> Result<()> {
        let (tx, rx) = mpsc::channel(100);
        self.rx = Some(rx);

        let handler = DiscordHandler {
            tx,
            config: self.config.clone(),
            bot_id: Arc::new(RwLock::new(None)),
        };

        let intents = GatewayIntents::GUILDS
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let mut client = Client::builder(&self.token, intents)
            .event_handler(handler)
            .await
            .map_err(|e| ServitorError::Comms {
                reason: format!("failed to create Discord client: {}", e),
            })?;

        let handle = tokio::spawn(async move {
            if let Err(e) = client.start().await {
                error!("Discord client error: {}", e);
            }
        });

        self.client_handle = Some(handle);
        info!("Discord transport connected");
        Ok(())
    }

    async fn recv(&mut self) -> Option<(CommsMessage, Box<dyn CommsResponder>)> {
        if let Some(ref mut rx) = self.rx {
            rx.recv()
                .await
                .map(|(msg, responder)| (msg, Box::new(responder) as Box<dyn CommsResponder>))
        } else {
            None
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(handle) = self.client_handle.take() {
            handle.abort();
        }
        self.rx = None;
        info!("Discord transport disconnected");
        Ok(())
    }
}

/// Internal Discord event handler.
struct DiscordHandler {
    tx: mpsc::Sender<(CommsMessage, DiscordResponder)>,
    config: DiscordConfig,
    bot_id: Arc<RwLock<Option<UserId>>>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Discord bot ready: {}", ready.user.name);
        // Store the bot ID
        let mut bot_id = self.bot_id.write().await;
        *bot_id = Some(ready.user.id);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Log ALL incoming messages at trace level
        tracing::trace!(
            author = %msg.author.name,
            author_id = %msg.author.id,
            guild_id = ?msg.guild_id,
            content_len = msg.content.len(),
            "received Discord message event"
        );

        // Ignore bot messages
        if msg.author.bot {
            tracing::trace!("ignoring bot message");
            return;
        }

        // Check guild allowlist
        if let Some(guild_id) = msg.guild_id {
            if !self.config.guild_allowlist.is_empty()
                && !self.config.guild_allowlist.contains(&guild_id.to_string())
            {
                debug!("Ignoring message from non-allowlisted guild: {}", guild_id);
                return;
            }
        }

        // User authorization is handled by Authority in main.rs

        // Get bot ID
        let bot_id = {
            let id = self.bot_id.read().await;
            match *id {
                Some(id) => id,
                None => {
                    debug!("Bot ID not yet available");
                    return;
                }
            }
        };

        // Check mention requirement
        let is_mentioned = msg.mentions_user_id(bot_id);
        let is_dm = msg.guild_id.is_none();

        tracing::trace!(
            is_mentioned = is_mentioned,
            is_dm = is_dm,
            require_mention = self.config.require_mention,
            "checking mention requirement"
        );

        if self.config.require_mention && !is_mentioned && !is_dm {
            tracing::trace!("ignoring: mention required but not mentioned");
            return;
        }

        // Strip the bot mention from content
        let content = if is_mentioned {
            msg.content
                .replace(&format!("<@{}>", bot_id), "")
                .replace(&format!("<@!{}>", bot_id), "")
                .trim()
                .to_string()
        } else {
            msg.content.clone()
        };

        // Skip empty messages
        if content.is_empty() {
            tracing::trace!("ignoring empty message after mention strip");
            return;
        }

        tracing::debug!(
            user = %msg.author.name,
            content_len = content.len(),
            "Discord message passed all filters"
        );

        // Build guild info (we can get guild name via HTTP if needed, for now use ID)
        let (guild_id, guild_name) = if let Some(gid) = msg.guild_id {
            (gid.to_string(), gid.to_string())
        } else {
            ("dm".to_string(), "Direct Message".to_string())
        };

        let comms_msg = CommsMessage {
            source: CommsSource::Discord { guild_id, guild_name },
            channel_id: msg.channel_id.to_string(),
            user_id: msg.author.id.to_string(),
            user_name: msg.author.name.clone(),
            content,
            reply_to: msg.referenced_message.as_ref().map(|m| m.id.to_string()),
            message_id: msg.id.to_string(),
            timestamp: Utc::now(),
        };

        let responder = DiscordResponder {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            message_id: msg.id,
        };

        if let Err(e) = self.tx.send((comms_msg, responder)).await {
            error!("Failed to send message to channel: {}", e);
        }
    }
}

/// Responder for Discord messages.
#[derive(Clone)]
pub struct DiscordResponder {
    http: Arc<Http>,
    channel_id: ChannelId,
    message_id: MessageId,
}

#[async_trait]
impl CommsResponder for DiscordResponder {
    async fn send(&self, response: CommsResponse) -> Result<()> {
        let channel_id = if response.channel_id.is_empty() {
            self.channel_id
        } else {
            ChannelId::new(response.channel_id.parse().map_err(|_| ServitorError::Comms {
                reason: format!("invalid channel ID: {}", response.channel_id),
            })?)
        };

        // Split long messages (Discord limit is 2000 chars)
        let chunks = split_message(&response.content, 2000);

        for (i, chunk) in chunks.iter().enumerate() {
            let mut builder = CreateMessage::new().content(chunk);

            // Reply to original message on first chunk
            if i == 0 {
                builder = builder.reference_message((channel_id, self.message_id));
            }

            channel_id
                .send_message(&self.http, builder)
                .await
                .map_err(|e| ServitorError::Comms {
                    reason: format!("failed to send Discord message: {}", e),
                })?;
        }

        Ok(())
    }
}

/// Split a message into chunks respecting Discord's character limit.
fn split_message(content: &str, max_len: usize) -> Vec<String> {
    if content.len() <= max_len {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in content.lines() {
        if current.len() + line.len() + 1 > max_len {
            if !current.is_empty() {
                chunks.push(current);
                current = String::new();
            }
            // Handle single lines longer than max
            if line.len() > max_len {
                let mut remaining = line;
                while remaining.len() > max_len {
                    chunks.push(remaining[..max_len].to_string());
                    remaining = &remaining[max_len..];
                }
                if !remaining.is_empty() {
                    current = remaining.to_string();
                }
            } else {
                current = line.to_string();
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_short_message() {
        let chunks = split_message("Hello, world!", 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello, world!");
    }

    #[test]
    fn split_long_message() {
        let content = "a".repeat(2500);
        let chunks = split_message(&content, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 500);
    }
}
