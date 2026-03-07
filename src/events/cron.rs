//! Cron event source — scheduled task execution.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule;

use crate::config::ScheduledTask;
use crate::egregore::Task;
use crate::error::{Result, ServitorError};
use crate::events::{task_from_schedule, EventSource};

/// Scheduled task with parsed cron expression.
struct ScheduledEntry {
    name: String,
    schedule: Schedule,
    task_prompt: String,
    publish: bool,
    notify: Option<String>,
    next_run: DateTime<Utc>,
}

impl ScheduledEntry {
    fn new(config: &ScheduledTask) -> Result<Self> {
        let schedule: Schedule = config.cron.parse().map_err(|e| ServitorError::Cron {
            reason: format!("invalid cron expression '{}': {}", config.cron, e),
        })?;

        let next_run = schedule
            .upcoming(Utc)
            .next()
            .ok_or_else(|| ServitorError::Cron {
                reason: format!("cron expression '{}' has no upcoming times", config.cron),
            })?;

        Ok(Self {
            name: config.name.clone(),
            schedule,
            task_prompt: config.task.clone(),
            publish: config.publish,
            notify: config.notify.clone(),
            next_run,
        })
    }

    fn update_next_run(&mut self) {
        if let Some(next) = self.schedule.upcoming(Utc).next() {
            self.next_run = next;
        }
    }

    fn is_due(&self) -> bool {
        Utc::now() >= self.next_run
    }
}

/// Cron-based event source.
pub struct CronSource {
    entries: Vec<ScheduledEntry>,
}

impl CronSource {
    /// Create a new cron source from scheduled task configurations.
    pub fn new(tasks: &[ScheduledTask]) -> Result<Self> {
        let mut entries = Vec::new();
        for task in tasks {
            let entry = ScheduledEntry::new(task)?;
            tracing::info!(
                name = %entry.name,
                next_run = %entry.next_run,
                "scheduled task registered"
            );
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// Check if any tasks are due and return them.
    pub fn check_due(&mut self) -> Vec<Task> {
        let mut due_tasks = Vec::new();

        for entry in &mut self.entries {
            if entry.is_due() {
                tracing::info!(name = %entry.name, "scheduled task triggered");

                let mut context = HashMap::new();
                context.insert(
                    "source".to_string(),
                    serde_json::json!("scheduled"),
                );
                context.insert(
                    "schedule_name".to_string(),
                    serde_json::json!(entry.name),
                );
                context.insert(
                    "publish".to_string(),
                    serde_json::json!(entry.publish),
                );
                if let Some(ref notify) = entry.notify {
                    context.insert("notify".to_string(), serde_json::json!(notify));
                }

                let task = task_from_schedule(&entry.name, &entry.task_prompt, context);
                due_tasks.push(task);

                entry.update_next_run();
                tracing::debug!(name = %entry.name, next_run = %entry.next_run, "next execution");
            }
        }

        due_tasks
    }

    /// Get the time until the next scheduled task.
    pub fn time_until_next(&self) -> Option<std::time::Duration> {
        self.entries
            .iter()
            .map(|e| e.next_run)
            .min()
            .map(|next| {
                let now = Utc::now();
                if next > now {
                    (next - now).to_std().unwrap_or(std::time::Duration::ZERO)
                } else {
                    std::time::Duration::ZERO
                }
            })
    }
}

#[async_trait]
impl EventSource for CronSource {
    async fn next(&mut self) -> Option<Task> {
        self.check_due().into_iter().next()
    }

    fn name(&self) -> &str {
        "cron"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_cron() {
        let task = ScheduledTask {
            name: "test".to_string(),
            cron: "0 * * * * *".to_string(), // Every minute
            task: "Test task".to_string(),
            publish: false,
            notify: None,
        };

        let source = CronSource::new(&[task]).unwrap();
        assert_eq!(source.entries.len(), 1);
    }

    #[test]
    fn reject_invalid_cron() {
        let task = ScheduledTask {
            name: "test".to_string(),
            cron: "not a cron".to_string(),
            task: "Test task".to_string(),
            publish: false,
            notify: None,
        };

        let result = CronSource::new(&[task]);
        assert!(result.is_err());
    }

    #[test]
    fn time_until_next_calculation() {
        let task = ScheduledTask {
            name: "test".to_string(),
            cron: "0 0 * * * *".to_string(), // Every hour
            task: "Test task".to_string(),
            publish: false,
            notify: None,
        };

        let source = CronSource::new(&[task]).unwrap();
        let duration = source.time_until_next();
        assert!(duration.is_some());
    }
}
