//! Task capability filtering.

use std::collections::HashSet;

use crate::egregore::Task;

/// Check if a task's required capabilities are satisfied by the given capability set.
///
/// Returns `true` if all required capabilities are present, or if the task
/// has no required capabilities.
pub fn task_matches_capabilities(task: &Task, capability_set: &HashSet<String>) -> bool {
    if task.required_caps.is_empty() {
        return true;
    }

    task.required_caps
        .iter()
        .all(|capability| capability_set.contains(capability))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_task_with_caps(caps: Vec<&str>) -> Task {
        Task {
            msg_type: "task".to_string(),
            id: Some("test-1".to_string()),
            hash: "hash-1".to_string(),
            task_type: None,
            request: None,
            requestor: None,
            prompt: "test prompt".to_string(),
            required_caps: caps.into_iter().map(String::from).collect(),
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: None,
            author: None,
            keeper: None,
            scope_override: None,
            tool_calls: vec![],
        }
    }

    #[test]
    fn empty_caps_always_matches() {
        let task = test_task_with_caps(vec![]);
        let caps: HashSet<String> = HashSet::new();
        assert!(task_matches_capabilities(&task, &caps));
    }

    #[test]
    fn matches_when_all_caps_present() {
        let task = test_task_with_caps(vec!["docker", "shell"]);
        let caps: HashSet<String> = ["docker", "shell", "git"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(task_matches_capabilities(&task, &caps));
    }

    #[test]
    fn fails_when_cap_missing() {
        let task = test_task_with_caps(vec!["docker", "kubernetes"]);
        let caps: HashSet<String> = ["docker", "shell"].into_iter().map(String::from).collect();
        assert!(!task_matches_capabilities(&task, &caps));
    }
}
