use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::config::TaskConfig;
use crate::egregore::{Task, TaskAssign, TaskFailed, TaskOffer, TaskOfferWithdraw, TaskStarted};
use crate::identity::PublicId;

#[derive(Debug, Clone)]
pub struct OfferDecision {
    pub task: Task,
    pub requestor: PublicId,
    pub offer: TaskOffer,
}

#[derive(Debug, Clone)]
pub struct AssignmentDecision {
    pub task: Task,
    pub requestor: PublicId,
    pub started: TaskStarted,
}

#[derive(Debug, Clone)]
pub struct OfferedTask {
    pub task: Task,
    pub requestor: PublicId,
    pub offered_at: Instant,
}

#[derive(Debug, Clone)]
pub struct ActiveExecution {
    pub task: Task,
    pub requestor: PublicId,
    pub started_at: Instant,
}

#[derive(Debug, Clone)]
pub struct ExpiredOffer {
    pub task: Task,
    pub requestor: PublicId,
    pub expired_at: Instant,
}

#[derive(Debug, Clone)]
pub enum TaskLifecycleEvent {
    Withdraw(TaskOfferWithdraw),
    Failed(TaskFailed),
}

pub struct TaskCoordinator {
    servitor: PublicId,
    config: TaskConfig,
    offered: HashMap<String, OfferedTask>,
    active: HashMap<String, ActiveExecution>,
    queued_assignments: VecDeque<AssignmentDecision>,
}

impl TaskCoordinator {
    pub fn new(servitor: PublicId, config: TaskConfig) -> Self {
        Self {
            servitor,
            config,
            offered: HashMap::new(),
            active: HashMap::new(),
            queued_assignments: VecDeque::new(),
        }
    }

    pub fn register_offer(
        &mut self,
        task: Task,
        requestor: PublicId,
        capabilities: Vec<String>,
    ) -> OfferDecision {
        let task_id = task.effective_id().to_string();
        let offer = TaskOffer::new(
            task_id.clone(),
            self.servitor.clone(),
            capabilities,
            self.config.offer_ttl_secs,
        );
        self.offered.insert(
            task_id,
            OfferedTask {
                task: task.clone(),
                requestor: requestor.clone(),
                offered_at: Instant::now(),
            },
        );
        OfferDecision {
            task,
            requestor,
            offer,
        }
    }

    pub fn pending_requestor(&self, task_id: &str) -> Option<&PublicId> {
        self.offered.get(task_id).map(|offered| &offered.requestor)
    }

    pub fn pending_task(&self, task_id: &str) -> Option<&Task> {
        self.offered.get(task_id).map(|offered| &offered.task)
    }

    pub fn apply_assignment(
        &mut self,
        assign: &TaskAssign,
        now: Instant,
        eta_seconds: u64,
    ) -> Option<AssignmentDecision> {
        if assign.servitor != self.servitor {
            return None;
        }

        let offered = self.offered.remove(&assign.task_id)?;
        let started = TaskStarted::new(assign.task_id.clone(), self.servitor.clone(), eta_seconds);
        self.active.insert(
            assign.task_id.clone(),
            ActiveExecution {
                task: offered.task.clone(),
                requestor: offered.requestor.clone(),
                started_at: now,
            },
        );

        Some(AssignmentDecision {
            task: offered.task,
            requestor: offered.requestor,
            started,
        })
    }

    pub fn finish_execution(&mut self, task_id: &str) -> Option<ActiveExecution> {
        self.active.remove(task_id)
    }

    pub fn has_active_execution(&self) -> bool {
        !self.active.is_empty()
    }

    pub fn enqueue_assignment(&mut self, decision: AssignmentDecision) {
        self.queued_assignments.push_back(decision);
    }

    pub fn take_next_assignment(&mut self) -> Option<AssignmentDecision> {
        self.queued_assignments.pop_front()
    }

    pub fn collect_timeouts(&mut self, now: Instant) -> Vec<TaskLifecycleEvent> {
        let ttl = Duration::from_secs(self.config.offer_ttl_secs);
        let expired_ids: Vec<String> = self
            .offered
            .iter()
            .filter_map(|(task_id, offered)| {
                (offered.offered_at + ttl <= now).then_some(task_id.clone())
            })
            .collect();

        expired_ids
            .into_iter()
            .filter_map(|task_id| {
                self.offered.remove(&task_id).map(|offered| {
                    TaskLifecycleEvent::Withdraw(TaskOfferWithdraw::new(
                        task_id,
                        self.servitor.clone(),
                        Some(format!("offer expired for requestor {}", offered.requestor)),
                    ))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servitor_id() -> PublicId {
        PublicId("@SERVITORAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string())
    }

    fn task() -> Task {
        Task {
            msg_type: "task".to_string(),
            id: Some("task-1".to_string()),
            hash: "hash-1".to_string(),
            task_type: Some("docker:deploy".to_string()),
            request: Some("deploy".to_string()),
            requestor: Some(PublicId(
                "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string(),
            )),
            prompt: "deploy".to_string(),
            required_caps: vec![],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: Some(60),
            author: None,
            keeper: None,
        }
    }

    #[test]
    fn offer_registration_tracks_pending_task() {
        let mut coordinator = TaskCoordinator::new(servitor_id(), TaskConfig::default());
        let task = task();
        let requestor = task.requestor.clone().unwrap();
        let decision =
            coordinator.register_offer(task.clone(), requestor.clone(), vec!["docker".to_string()]);
        assert_eq!(decision.offer.task_id, "task-1");
        assert_eq!(decision.requestor, requestor);
        assert!(coordinator.pending_task("task-1").is_some());
    }

    #[test]
    fn assignment_moves_task_to_active_execution() {
        let mut coordinator = TaskCoordinator::new(servitor_id(), TaskConfig::default());
        let task = task();
        let requestor = task.requestor.clone().unwrap();
        coordinator.register_offer(task, requestor, vec!["docker".to_string()]);

        let assign = TaskAssign {
            msg_type: "task_assign".to_string(),
            task_id: "task-1".to_string(),
            servitor: servitor_id(),
            assigner: None,
        };

        let decision = coordinator
            .apply_assignment(&assign, Instant::now(), 120)
            .unwrap();
        assert_eq!(decision.started.task_id, "task-1");
        assert!(coordinator.pending_task("task-1").is_none());
        assert!(coordinator.finish_execution("task-1").is_some());
    }

    #[test]
    fn timeout_collection_withdraws_expired_offers() {
        let mut coordinator = TaskCoordinator::new(
            servitor_id(),
            TaskConfig {
                offer_ttl_secs: 1,
                ..TaskConfig::default()
            },
        );
        let task = task();
        let requestor = task.requestor.clone().unwrap();
        coordinator.register_offer(task, requestor, vec!["docker".to_string()]);

        let events = coordinator.collect_timeouts(Instant::now() + Duration::from_secs(2));
        assert!(matches!(
            events.first(),
            Some(TaskLifecycleEvent::Withdraw(_))
        ));
    }
}
