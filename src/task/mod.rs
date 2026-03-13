//! Task coordination helpers for offer -> assign -> execute flow.

mod state;

pub use state::{
    ActiveExecution, AssignmentDecision, ExpiredOffer, OfferDecision, OfferedTask, TaskCoordinator,
    TaskLifecycleEvent,
};

use crate::authority::{AuthRequest, AuthResult, Authority, PersonId};
use crate::egregore::Task;
use crate::identity::PublicId;

/// Build the request authorization skill string for a task.
pub fn request_skill(task: &Task) -> String {
    format!("request:{}", task.effective_task_type())
}

/// Build the assignment authorization skill string for a task.
pub fn assign_skill(task: &Task) -> String {
    format!("assign:{}", task.effective_task_type())
}

/// Check whether a requestor is allowed to ask for this task type.
pub fn authorize_offer_request(
    authority: &Authority,
    requestor: &PublicId,
    task: &Task,
) -> AuthResult {
    authority.authorize(&AuthRequest {
        person: PersonId::from_egregore(&requestor.0),
        place: "egregore:local".to_string(),
        skill: request_skill(task),
    })
}

/// Check whether an assigner may assign work for this task.
pub fn authorize_assignment(
    authority: &Authority,
    assigner: &PublicId,
    requestor: &PublicId,
    task: &Task,
) -> bool {
    if assigner == requestor {
        return true;
    }

    authority
        .authorize(&AuthRequest {
            person: PersonId::from_egregore(&assigner.0),
            place: "egregore:local".to_string(),
            skill: assign_skill(task),
        })
        .allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::AuthorityConfig;

    fn test_authority() -> Authority {
        Authority::from_config(
            AuthorityConfig::from_toml(
                r#"
[[keeper]]
name = "requestor"
egregore = "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519"

[[keeper]]
name = "assigner"
egregore = "@BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=.ed25519"

[[permission]]
keeper = "requestor"
place = "*"
skills = ["request:docker:*"]

[[permission]]
keeper = "assigner"
place = "*"
skills = ["assign:*"]
"#,
            )
            .unwrap(),
        )
    }

    fn test_task() -> Task {
        Task {
            msg_type: "task".to_string(),
            id: Some("task-1".to_string()),
            hash: "hash-1".to_string(),
            task_type: Some("docker:deploy:staging".to_string()),
            request: Some("deploy staging".to_string()),
            requestor: Some(PublicId(
                "@AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=.ed25519".to_string(),
            )),
            prompt: "deploy staging".to_string(),
            required_caps: vec!["docker".to_string()],
            parent_id: None,
            context: Default::default(),
            priority: 0,
            timeout_secs: Some(60),
            author: None,
            keeper: None,
        }
    }

    #[test]
    fn offer_gate_uses_request_permission() {
        let auth = test_authority();
        let task = test_task();
        let result = authorize_offer_request(&auth, task.requestor.as_ref().unwrap(), &task);
        assert!(result.allowed);
    }

    #[test]
    fn assignment_allows_original_requestor() {
        let auth = test_authority();
        let task = test_task();
        let requestor = task.requestor.as_ref().unwrap();
        assert!(authorize_assignment(&auth, requestor, requestor, &task));
    }

    #[test]
    fn assignment_requires_assign_permission_for_others() {
        let auth = test_authority();
        let task = test_task();
        let requestor = task.requestor.as_ref().unwrap();
        let assigner =
            PublicId("@BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=.ed25519".to_string());
        assert!(authorize_assignment(&auth, &assigner, requestor, &task));

        let stranger =
            PublicId("@CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=.ed25519".to_string());
        assert!(!authorize_assignment(&auth, &stranger, requestor, &task));
    }
}
