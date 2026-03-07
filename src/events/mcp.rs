//! MCP notification event source — server-pushed events.
//!
//! Handles notifications from MCP servers and converts them to tasks.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::egregore::Task;
use crate::events::{task_from_schedule, EventSource};

/// MCP notification converted to a pending task.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub server_name: String,
    pub method: String,
    pub params: serde_json::Value,
    pub task_template: String,
}

/// MCP notification event source.
pub struct McpNotificationSource {
    /// Pending notifications to process.
    pending: Vec<PendingNotification>,
    /// Task templates per server (from config).
    templates: HashMap<String, String>,
}

impl McpNotificationSource {
    /// Create a new MCP notification source.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            templates: HashMap::new(),
        }
    }

    /// Register a notification handler for an MCP server.
    pub fn register_handler(&mut self, server_name: &str, task_template: &str) {
        self.templates
            .insert(server_name.to_string(), task_template.to_string());
        tracing::debug!(
            server = server_name,
            "registered MCP notification handler"
        );
    }

    /// Queue a notification for processing.
    pub fn queue_notification(
        &mut self,
        server_name: &str,
        method: &str,
        params: serde_json::Value,
    ) {
        if let Some(template) = self.templates.get(server_name) {
            self.pending.push(PendingNotification {
                server_name: server_name.to_string(),
                method: method.to_string(),
                params,
                task_template: template.clone(),
            });
            tracing::debug!(
                server = server_name,
                method = method,
                "queued MCP notification"
            );
        } else {
            tracing::trace!(
                server = server_name,
                method = method,
                "ignoring notification (no handler)"
            );
        }
    }

    /// Convert a notification to a task.
    fn notification_to_task(&self, notification: &PendingNotification) -> Task {
        // Interpolate {{notification}} in template
        let prompt = notification
            .task_template
            .replace("{{notification}}", &notification.params.to_string());

        let mut context = HashMap::new();
        context.insert("source".to_string(), serde_json::json!("mcp_notification"));
        context.insert(
            "mcp_server".to_string(),
            serde_json::json!(notification.server_name),
        );
        context.insert(
            "method".to_string(),
            serde_json::json!(notification.method),
        );
        context.insert("params".to_string(), notification.params.clone());

        task_from_schedule(&notification.server_name, &prompt, context)
    }
}

impl Default for McpNotificationSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventSource for McpNotificationSource {
    async fn next(&mut self) -> Option<Task> {
        self.pending.pop().map(|n| self.notification_to_task(&n))
    }

    fn name(&self) -> &str {
        "mcp_notification"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_queuing() {
        let mut source = McpNotificationSource::new();
        source.register_handler("shell", "Handle event: {{notification}}");

        source.queue_notification(
            "shell",
            "file_changed",
            serde_json::json!({"path": "/tmp/test.txt"}),
        );

        assert_eq!(source.pending.len(), 1);
    }

    #[test]
    fn notification_to_task_conversion() {
        let mut source = McpNotificationSource::new();
        source.register_handler("shell", "Handle event: {{notification}}");

        source.queue_notification(
            "shell",
            "file_changed",
            serde_json::json!({"path": "/tmp/test.txt"}),
        );

        let notification = source.pending.pop().unwrap();
        let task = source.notification_to_task(&notification);

        assert!(task.prompt.contains("/tmp/test.txt"));
    }

    #[test]
    fn unregistered_server_ignored() {
        let mut source = McpNotificationSource::new();
        // No handler registered for "docker"

        source.queue_notification("docker", "container_started", serde_json::json!({}));

        assert_eq!(source.pending.len(), 0);
    }
}
