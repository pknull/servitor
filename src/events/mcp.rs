//! MCP notification event source — server-pushed events.
//!
//! Handles notifications from MCP servers and converts them to tasks.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::config::{ToolCallTemplate, WatchConfig};
use crate::egregore::Task;
use crate::events::{task_from_template, EventSource};

#[derive(Debug, Clone)]
struct NotificationRoute {
    route_name: String,
    event: Option<String>,
    filter: HashMap<String, serde_json::Value>,
    prompt: Option<String>,
    tool_calls: Vec<ToolCallTemplate>,
    notify: Option<String>,
}

/// MCP notification converted to a pending task.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub server_name: String,
    pub method: String,
    pub params: serde_json::Value,
    pub route_name: String,
    pub prompt: Option<String>,
    pub tool_calls: Vec<ToolCallTemplate>,
    pub notify: Option<String>,
}

/// MCP notification event source.
pub struct McpNotificationSource {
    /// Pending notifications to process.
    pending: Vec<PendingNotification>,
    /// Task templates per server (from config).
    routes: HashMap<String, Vec<NotificationRoute>>,
}

impl McpNotificationSource {
    /// Create a new MCP notification source.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            routes: HashMap::new(),
        }
    }

    /// Register a notification handler for an MCP server.
    pub fn register_handler(&mut self, server_name: &str, tool_calls: &[ToolCallTemplate]) {
        self.routes
            .entry(server_name.to_string())
            .or_default()
            .push(NotificationRoute {
                route_name: server_name.to_string(),
                event: None,
                filter: HashMap::new(),
                prompt: None,
                tool_calls: tool_calls.to_vec(),
                notify: None,
            });
        tracing::debug!(server = server_name, "registered MCP notification handler");
    }

    /// Register a watcher route for an MCP server.
    pub fn register_watch(&mut self, watch: &WatchConfig) {
        self.routes
            .entry(watch.mcp.clone())
            .or_default()
            .push(NotificationRoute {
                route_name: watch.name.clone(),
                event: Some(watch.event.clone()),
                filter: watch.filter.clone(),
                prompt: watch.prompt.clone(),
                tool_calls: watch.tool_calls.clone(),
                notify: watch.notify.clone(),
            });
        tracing::debug!(server = %watch.mcp, watch = %watch.name, "registered MCP watcher");
    }

    /// Queue a notification for processing.
    pub fn queue_notification(
        &mut self,
        server_name: &str,
        method: &str,
        params: serde_json::Value,
    ) {
        if let Some(routes) = self.routes.get(server_name) {
            let mut queued = 0usize;
            for route in routes {
                if !route_matches(route, method, &params) {
                    continue;
                }

                self.pending.push(PendingNotification {
                    server_name: server_name.to_string(),
                    method: method.to_string(),
                    params: params.clone(),
                    route_name: route.route_name.clone(),
                    prompt: route.prompt.clone().or_else(|| {
                        Some(format!(
                            "Handle MCP notification '{method}' from '{server_name}'"
                        ))
                    }),
                    tool_calls: route.tool_calls.clone(),
                    notify: route.notify.clone(),
                });
                queued += 1;
            }

            if queued > 0 {
                tracing::debug!(
                    server = server_name,
                    method = method,
                    queued,
                    "queued MCP notification"
                );
            } else {
                tracing::trace!(
                    server = server_name,
                    method = method,
                    "ignoring notification (no matching route)"
                );
            }
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
        let mut context = HashMap::new();
        context.insert("source".to_string(), serde_json::json!("mcp_notification"));
        context.insert(
            "mcp_server".to_string(),
            serde_json::json!(notification.server_name),
        );
        context.insert(
            "watch_name".to_string(),
            serde_json::json!(notification.route_name),
        );
        context.insert("method".to_string(), serde_json::json!(notification.method));
        context.insert("params".to_string(), notification.params.clone());
        if let Some(ref notify) = notification.notify {
            context.insert("notify".to_string(), serde_json::json!(notify));
        }

        let tool_calls = notification
            .tool_calls
            .iter()
            .map(|call| ToolCallTemplate {
                name: call.name.clone(),
                arguments: interpolate_value(&call.arguments, notification),
            })
            .collect::<Vec<_>>();

        task_from_template(
            &notification.route_name,
            notification.prompt.as_deref(),
            &tool_calls,
            context,
        )
    }
}

fn route_matches(route: &NotificationRoute, method: &str, params: &serde_json::Value) -> bool {
    if let Some(ref route_event) = route.event {
        if route_event != method {
            return false;
        }
    }

    route.filter.iter().all(|(key, expected)| {
        params
            .as_object()
            .and_then(|object| object.get(key))
            .map(|actual| actual == expected)
            .unwrap_or(false)
    })
}

fn interpolate_value(
    value: &serde_json::Value,
    notification: &PendingNotification,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) if text == "{{notification}}" => {
            notification.params.clone()
        }
        serde_json::Value::String(text) if text == "{{method}}" => {
            serde_json::Value::String(notification.method.clone())
        }
        serde_json::Value::String(text) if text == "{{server}}" => {
            serde_json::Value::String(notification.server_name.clone())
        }
        serde_json::Value::String(text) => serde_json::Value::String(
            text.replace("{{notification}}", &notification.params.to_string())
                .replace("{{method}}", &notification.method)
                .replace("{{server}}", &notification.server_name),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| interpolate_value(item, notification))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), interpolate_value(item, notification)))
                .collect(),
        ),
        other => other.clone(),
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
        source.register_handler(
            "shell",
            &[ToolCallTemplate {
                name: "shell__execute".to_string(),
                arguments: serde_json::json!({"command": "echo {{notification}}"}),
            }],
        );

        source.queue_notification(
            "shell",
            "file_changed",
            serde_json::json!({"path": "/tmp/test.txt"}),
        );

        assert_eq!(source.pending.len(), 1);
    }

    #[test]
    fn watcher_filters_notifications() {
        let mut source = McpNotificationSource::new();
        source.register_watch(&WatchConfig {
            name: "important-email".to_string(),
            mcp: "email".to_string(),
            event: "new_message".to_string(),
            filter: HashMap::from([("priority".to_string(), serde_json::json!("high"))]),
            prompt: Some("Summarize priority email".to_string()),
            tool_calls: vec![ToolCallTemplate {
                name: "email__summarize_message".to_string(),
                arguments: serde_json::json!({"message": "{{notification}}"}),
            }],
            notify: Some("egregore:channel:alerts".to_string()),
        });

        source.queue_notification(
            "email",
            "new_message",
            serde_json::json!({"priority":"low","body":"ignore"}),
        );
        source.queue_notification(
            "email",
            "new_message",
            serde_json::json!({"priority":"high","body":"keep"}),
        );

        assert_eq!(source.pending.len(), 1);
        assert_eq!(source.pending[0].route_name, "important-email");
    }

    #[test]
    fn notification_to_task_conversion() {
        let mut source = McpNotificationSource::new();
        source.register_handler(
            "shell",
            &[ToolCallTemplate {
                name: "shell__execute".to_string(),
                arguments: serde_json::json!({"command": "echo {{notification}}"}),
            }],
        );

        source.queue_notification(
            "shell",
            "file_changed",
            serde_json::json!({"path": "/tmp/test.txt"}),
        );

        let notification = source.pending.pop().unwrap();
        let task = source.notification_to_task(&notification);

        assert_eq!(task.tool_calls.len(), 1);
        assert_eq!(
            task.tool_calls[0].arguments,
            serde_json::json!({"command": "echo {\"path\":\"/tmp/test.txt\"}"})
        );
    }

    #[test]
    fn unregistered_server_ignored() {
        let mut source = McpNotificationSource::new();
        // No handler registered for "docker"

        source.queue_notification("docker", "container_started", serde_json::json!({}));

        assert_eq!(source.pending.len(), 0);
    }
}
