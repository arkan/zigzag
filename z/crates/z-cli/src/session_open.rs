use z_core::domain::{Project, Session};

/// Plan for opening a Project Session.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenSessionPlan {
    pub target_session: Session,
    pub existing_session: Option<Session>,
}

/// Decide whether opening a Project branch should attach an existing Session or create one.
pub fn plan_open_session(project: &Project, branch: &str, live_sessions: &[Session]) -> OpenSessionPlan {
    let target_session = Session::new(&project.name, branch);
    let existing_session = live_sessions
        .iter()
        .find(|session| session.name == target_session.name)
        .cloned();
    OpenSessionPlan {
        target_session,
        existing_session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project() -> Project {
        Project {
            name: "myapp".to_string(),
            path: PathBuf::from("/repo/myapp"),
            host: None,
            transport: None,
        }
    }

    #[test]
    fn plans_attach_when_target_session_exists() {
        let sessions = vec![Session::new("myapp", "main")];
        let plan = plan_open_session(&project(), "main", &sessions);

        assert_eq!(plan.target_session.name, "myapp:main");
        assert_eq!(
            plan.existing_session.as_ref().map(|s| s.name.as_str()),
            Some("myapp:main")
        );
    }

    #[test]
    fn plans_create_when_target_session_is_missing() {
        let sessions = vec![Session::new("myapp", "other")];
        let plan = plan_open_session(&project(), "main", &sessions);

        assert_eq!(plan.target_session.name, "myapp:main");
        assert!(plan.existing_session.is_none());
    }

    #[test]
    fn sanitizes_branch_name_for_target_session() {
        let plan = plan_open_session(&project(), "feat/login", &[]);

        assert_eq!(plan.target_session.name, "myapp:feat-login");
    }
}
