//! A2A server task state management.
//!
//! Provides in-memory task storage for tracking A2A tasks received from
//! external agents. Tasks flow through states: submitted → working → completed/failed/cancelled.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::a2a::client::{TaskResult, TaskState};

/// Server-side task representation.
#[derive(Debug, Clone)]
pub struct A2aServerTask {
    /// Unique task identifier.
    pub id: String,

    /// Skill being executed.
    pub skill: String,

    /// Input provided to the skill.
    pub input: serde_json::Value,

    /// Current task state.
    pub state: TaskState,

    /// Task result (if completed).
    pub result: Option<TaskResult>,

    /// Error message (if failed).
    pub error: Option<String>,

    /// Keeper name (if authenticated).
    pub keeper: Option<String>,

    /// When the task was created.
    pub created_at: Instant,

    /// When the task was last updated.
    pub updated_at: Instant,
}

impl A2aServerTask {
    /// Create a new task.
    pub fn new(id: String, skill: String, input: serde_json::Value, keeper: Option<String>) -> Self {
        let now = Instant::now();
        Self {
            id,
            skill,
            input,
            state: TaskState::Submitted,
            result: None,
            error: None,
            keeper,
            created_at: now,
            updated_at: now,
        }
    }

    /// Check if the task has timed out.
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        self.created_at.elapsed() > timeout
    }

    /// Mark as working.
    pub fn start(&mut self) {
        self.state = TaskState::Working;
        self.updated_at = Instant::now();
    }

    /// Mark as completed with result. Returns false if already terminal.
    pub fn complete(&mut self, result: TaskResult) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = TaskState::Completed;
        self.result = Some(result);
        self.updated_at = Instant::now();
        true
    }

    /// Mark as failed with error. Returns false if already terminal.
    pub fn fail(&mut self, error: String) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = TaskState::Failed;
        self.error = Some(error);
        self.updated_at = Instant::now();
        true
    }

    /// Mark as cancelled. Returns false if already terminal.
    pub fn cancel(&mut self) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = TaskState::Cancelled;
        self.updated_at = Instant::now();
        true
    }

    /// Convert to client-facing A2aTask representation.
    pub fn to_client_task(&self) -> crate::a2a::client::A2aTask {
        crate::a2a::client::A2aTask {
            id: self.id.clone(),
            state: self.state,
            result: self.result.clone(),
            error: self.error.clone(),
            artifacts: self
                .result
                .as_ref()
                .map(|r| r.artifacts.clone())
                .unwrap_or_default(),
        }
    }
}

/// In-memory task store with bounded capacity.
#[derive(Clone)]
pub struct A2aTaskStore {
    inner: Arc<RwLock<TaskStoreInner>>,
}

struct TaskStoreInner {
    tasks: HashMap<String, A2aServerTask>,
    max_tasks: usize,
    task_timeout: Duration,
}

impl A2aTaskStore {
    /// Create a new task store.
    pub fn new(max_tasks: usize, task_timeout_secs: u64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(TaskStoreInner {
                tasks: HashMap::new(),
                max_tasks,
                task_timeout: Duration::from_secs(task_timeout_secs),
            })),
        }
    }

    /// Create a new task. Returns None if at capacity.
    pub async fn create(
        &self,
        skill: String,
        input: serde_json::Value,
        keeper: Option<String>,
    ) -> Option<A2aServerTask> {
        let mut inner = self.inner.write().await;

        // Clean up completed/failed/cancelled and timed out tasks
        let timeout = inner.task_timeout;
        inner.tasks.retain(|_, task| {
            !task.state.is_terminal() && !task.is_timed_out(timeout)
        });

        // Check capacity
        if inner.tasks.len() >= inner.max_tasks {
            return None;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let task = A2aServerTask::new(id.clone(), skill, input, keeper);
        inner.tasks.insert(id, task.clone());

        Some(task)
    }

    /// Get a task by ID.
    pub async fn get(&self, task_id: &str) -> Option<A2aServerTask> {
        let inner = self.inner.read().await;
        inner.tasks.get(task_id).cloned()
    }

    /// Mark a task as working.
    pub async fn start(&self, task_id: &str) -> Option<A2aServerTask> {
        let mut inner = self.inner.write().await;
        if let Some(task) = inner.tasks.get_mut(task_id) {
            task.start();
            Some(task.clone())
        } else {
            None
        }
    }

    /// Mark a task as completed. Returns None if task not found or already terminal.
    pub async fn complete(&self, task_id: &str, result: TaskResult) -> Option<A2aServerTask> {
        let mut inner = self.inner.write().await;
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.complete(result) {
                return Some(task.clone());
            }
        }
        None
    }

    /// Mark a task as failed. Returns None if task not found or already terminal.
    pub async fn fail(&self, task_id: &str, error: String) -> Option<A2aServerTask> {
        let mut inner = self.inner.write().await;
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.fail(error) {
                return Some(task.clone());
            }
        }
        None
    }

    /// Cancel a task. Returns None if task not found or already terminal.
    pub async fn cancel(&self, task_id: &str) -> Option<A2aServerTask> {
        let mut inner = self.inner.write().await;
        if let Some(task) = inner.tasks.get_mut(task_id) {
            if task.cancel() {
                return Some(task.clone());
            }
        }
        None
    }

    /// Get count of active (non-terminal) tasks.
    pub async fn active_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner
            .tasks
            .values()
            .filter(|t| !t.state.is_terminal())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_lifecycle() {
        let store = A2aTaskStore::new(10, 300);

        // Create task
        let task = store
            .create("test_skill".to_string(), serde_json::json!({}), None)
            .await
            .unwrap();
        assert_eq!(task.state, TaskState::Submitted);

        // Start task
        let task = store.start(&task.id).await.unwrap();
        assert_eq!(task.state, TaskState::Working);

        // Complete task
        let task = store
            .complete(&task.id, TaskResult::text("done"))
            .await
            .unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert!(task.result.is_some());
    }

    #[tokio::test]
    async fn test_task_capacity() {
        let store = A2aTaskStore::new(2, 300);

        // Create two tasks
        let t1 = store
            .create("skill1".to_string(), serde_json::json!({}), None)
            .await;
        let t2 = store
            .create("skill2".to_string(), serde_json::json!({}), None)
            .await;
        assert!(t1.is_some());
        assert!(t2.is_some());

        // Third should fail (at capacity)
        let t3 = store
            .create("skill3".to_string(), serde_json::json!({}), None)
            .await;
        assert!(t3.is_none());

        // Complete one
        store
            .complete(&t1.unwrap().id, TaskResult::text("done"))
            .await;

        // Now we can create another (completed task cleaned up)
        let t4 = store
            .create("skill4".to_string(), serde_json::json!({}), None)
            .await;
        assert!(t4.is_some());
    }

    #[tokio::test]
    async fn test_cancel_non_terminal() {
        let store = A2aTaskStore::new(10, 300);

        let task = store
            .create("skill".to_string(), serde_json::json!({}), None)
            .await
            .unwrap();

        // Cancel works on non-terminal
        let cancelled = store.cancel(&task.id).await;
        assert!(cancelled.is_some());
        assert_eq!(cancelled.unwrap().state, TaskState::Cancelled);

        // Cancel again returns None (already terminal)
        let again = store.cancel(&task.id).await;
        assert!(again.is_none());
    }
}
