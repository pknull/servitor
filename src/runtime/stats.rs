//! Runtime statistics tracking for the servitor daemon.

use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::egregore::{ServitorLoad, ServitorStats};
use crate::metrics::{self, TaskStatus as MetricsTaskStatus};

/// Tracks runtime statistics for the daemon.
///
/// This struct maintains counters and timestamps for monitoring
/// task execution and daemon health.
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    started_at: Instant,
    tasks_offered: u64,
    tasks_executing: u64,
    tasks_queued: u64,
    tasks_executed: u64,
    tasks_failed: u64,
    /// Timestamp of the last completed task.
    pub last_task_ts: Option<DateTime<Utc>>,
    task_start_time: Option<Instant>,
}

impl RuntimeStats {
    /// Create new runtime stats with current timestamp.
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            tasks_offered: 0,
            tasks_executing: 0,
            tasks_queued: 0,
            tasks_executed: 0,
            tasks_failed: 0,
            last_task_ts: None,
            task_start_time: None,
        }
    }

    /// Record that a task offer was received.
    pub fn record_task_offer(&mut self) {
        self.tasks_offered += 1;
        self.tasks_queued += 1;
    }

    /// Record that a queued task was discarded (expired, rejected, etc).
    pub fn discard_task(&mut self) {
        self.tasks_queued = self.tasks_queued.saturating_sub(1);
    }

    /// Record that a task has started execution.
    pub fn start_task(&mut self) {
        self.tasks_queued = self.tasks_queued.saturating_sub(1);
        self.tasks_executing += 1;
        self.task_start_time = Some(Instant::now());
        metrics::set_active_tasks(self.tasks_executing);
    }

    /// Record that a task has finished execution.
    pub fn finish_task(&mut self, success: bool, task_type: Option<&str>) {
        let duration = self.task_start_time.map(|t| t.elapsed().as_secs_f64());
        self.task_start_time = None;
        self.tasks_executing = self.tasks_executing.saturating_sub(1);
        metrics::set_active_tasks(self.tasks_executing);

        let task_type_str = task_type.unwrap_or("unknown");
        if success {
            self.tasks_executed += 1;
            metrics::record_task_complete(task_type_str, MetricsTaskStatus::Success);
        } else {
            self.tasks_failed += 1;
            metrics::record_task_complete(task_type_str, MetricsTaskStatus::Error);
        }

        if let Some(d) = duration {
            metrics::record_task_duration(task_type_str, d);
        }

        self.last_task_ts = Some(Utc::now());
    }

    /// Get the daemon uptime in seconds.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Get current load metrics.
    pub fn load(&self) -> ServitorLoad {
        ServitorLoad {
            tasks_executing: self.tasks_executing,
            tasks_queued: self.tasks_queued,
        }
    }

    /// Get cumulative statistics.
    pub fn stats(&self) -> ServitorStats {
        ServitorStats {
            tasks_offered: self.tasks_offered,
            tasks_executed: self.tasks_executed,
            tasks_failed: self.tasks_failed,
        }
    }
}

impl Default for RuntimeStats {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeStats {
    /// Set the started_at time. Primarily for testing.
    pub fn set_started_at(&mut self, instant: Instant) {
        self.started_at = instant;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stats_are_zeroed() {
        let stats = RuntimeStats::new();
        assert_eq!(stats.tasks_offered, 0);
        assert_eq!(stats.tasks_executing, 0);
        assert_eq!(stats.tasks_queued, 0);
        assert_eq!(stats.tasks_executed, 0);
        assert_eq!(stats.tasks_failed, 0);
        assert!(stats.last_task_ts.is_none());
    }

    #[test]
    fn record_offer_increments_counters() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        assert_eq!(stats.tasks_offered, 1);
        assert_eq!(stats.tasks_queued, 1);
    }

    #[test]
    fn discard_task_decrements_queued() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.discard_task();
        assert_eq!(stats.tasks_queued, 0);
        assert_eq!(stats.tasks_offered, 1); // offered count preserved
    }

    #[test]
    fn start_task_moves_from_queued_to_executing() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.start_task();
        assert_eq!(stats.tasks_queued, 0);
        assert_eq!(stats.tasks_executing, 1);
    }

    #[test]
    fn finish_task_updates_counters() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.start_task();
        stats.finish_task(true, Some("test"));
        assert_eq!(stats.tasks_executing, 0);
        assert_eq!(stats.tasks_executed, 1);
        assert_eq!(stats.tasks_failed, 0);
        assert!(stats.last_task_ts.is_some());
    }

    #[test]
    fn finish_task_failure_increments_failed() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.start_task();
        stats.finish_task(false, Some("test"));
        assert_eq!(stats.tasks_executed, 0);
        assert_eq!(stats.tasks_failed, 1);
    }

    #[test]
    fn load_returns_current_state() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.record_task_offer();
        stats.start_task();
        let load = stats.load();
        assert_eq!(load.tasks_executing, 1);
        assert_eq!(load.tasks_queued, 1);
    }

    #[test]
    fn stats_returns_cumulative_totals() {
        let mut stats = RuntimeStats::new();
        stats.record_task_offer();
        stats.start_task();
        stats.finish_task(true, None);
        stats.record_task_offer();
        stats.start_task();
        stats.finish_task(false, None);
        let s = stats.stats();
        assert_eq!(s.tasks_offered, 2);
        assert_eq!(s.tasks_executed, 1);
        assert_eq!(s.tasks_failed, 1);
    }
}
