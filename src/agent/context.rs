//! Conversation context management.

use crate::agent::provider::{ContentBlock, Message};

/// Manages conversation context for agent execution.
#[derive(Debug, Default)]
pub struct ConversationContext {
    messages: Vec<Message>,
}

impl ConversationContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self { messages: Vec::new() }
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
            .filter(|m| m.role == crate::agent::provider::Role::Assistant)
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
}
