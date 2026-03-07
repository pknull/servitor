//! Conversation context management.

use crate::agent::provider::{ContentBlock, Message, Role};
use crate::egregore::ConversationTurn;

/// Manages conversation context for agent execution.
#[derive(Debug, Default)]
pub struct ConversationContext {
    messages: Vec<Message>,
}

impl ConversationContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Create context from conversation history (fetched from egregore).
    pub fn from_history(turns: Vec<ConversationTurn>) -> Self {
        let mut ctx = Self::new();
        for turn in turns {
            match turn.role.as_str() {
                "user" => ctx.add_user_message(turn.content),
                "assistant" => ctx.add_assistant_message(vec![ContentBlock::text(turn.content)]),
                _ => {}
            }
        }
        ctx
    }

    /// Prepend history to the beginning of the context.
    pub fn prepend_history(&mut self, turns: Vec<ConversationTurn>) {
        let mut new_messages = Vec::new();
        for turn in turns {
            match turn.role.as_str() {
                "user" => new_messages.push(Message::user(turn.content)),
                "assistant" => {
                    new_messages.push(Message::assistant(vec![ContentBlock::text(turn.content)]))
                }
                _ => {}
            }
        }
        new_messages.append(&mut self.messages);
        self.messages = new_messages;
    }

    /// Add a user message.
    pub fn add_user_message(&mut self, text: impl Into<String>) {
        self.messages.push(Message::user(text));
    }

    /// Add an assistant message.
    pub fn add_assistant_message(&mut self, content: Vec<ContentBlock>) {
        self.messages.push(Message::assistant(content));
    }

    /// Add tool results as a user message.
    pub fn add_tool_results(&mut self, results: Vec<ContentBlock>) {
        self.messages.push(Message::tool_results(results));
    }

    /// Get all messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get the number of turns (assistant messages).
    pub fn turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count()
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Truncate to keep only the last N messages (for context management).
    pub fn truncate(&mut self, keep_last: usize) {
        if self.messages.len() > keep_last {
            let start = self.messages.len() - keep_last;
            self.messages = self.messages.split_off(start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tracking() {
        let mut ctx = ConversationContext::new();

        ctx.add_user_message("Hello");
        ctx.add_assistant_message(vec![ContentBlock::text("Hi there")]);
        ctx.add_user_message("How are you?");
        ctx.add_assistant_message(vec![ContentBlock::text("I'm good")]);

        assert_eq!(ctx.messages().len(), 4);
        assert_eq!(ctx.turn_count(), 2);
    }

    #[test]
    fn truncation() {
        let mut ctx = ConversationContext::new();

        for i in 0..10 {
            ctx.add_user_message(format!("Message {}", i));
        }

        ctx.truncate(5);
        assert_eq!(ctx.messages().len(), 5);
    }

    #[test]
    fn from_history() {
        let turns = vec![
            ConversationTurn {
                role: "user".to_string(),
                content: "Hello".to_string(),
                hash: "abc".to_string(),
            },
            ConversationTurn {
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
                hash: "def".to_string(),
            },
        ];

        let ctx = ConversationContext::from_history(turns);
        assert_eq!(ctx.messages().len(), 2);
        assert_eq!(ctx.turn_count(), 1);
    }

    #[test]
    fn prepend_history() {
        let mut ctx = ConversationContext::new();
        ctx.add_user_message("Current message");

        let turns = vec![ConversationTurn {
            role: "user".to_string(),
            content: "Previous message".to_string(),
            hash: "abc".to_string(),
        }];

        ctx.prepend_history(turns);
        assert_eq!(ctx.messages().len(), 2);
        // First message should be the prepended one
        if let ContentBlock::Text { text } = &ctx.messages()[0].content[0] {
            assert_eq!(text, "Previous message");
        }
    }
}
