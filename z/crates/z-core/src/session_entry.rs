use crate::activity::ActivityStore;

/// Best-effort effects applied when entering a Session.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionEntryEffects {
    pub notifications_cleared: bool,
    pub activity_recorded: bool,
}

/// Record that a Session was attached/opened.
pub fn record_session_attach(activity: &dyn ActivityStore, session: &str) -> SessionEntryEffects {
    SessionEntryEffects {
        notifications_cleared: false,
        activity_recorded: activity.record_attach(session).is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::SessionActivity;
    use crate::error::{Result, ZError};
    use std::cell::RefCell;

    struct FakeActivity {
        recorded: RefCell<Vec<String>>,
        fail: bool,
    }

    impl FakeActivity {
        fn ok() -> Self {
            Self {
                recorded: RefCell::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                recorded: RefCell::new(Vec::new()),
                fail: true,
            }
        }
    }

    impl ActivityStore for FakeActivity {
        fn record_attach(&self, session: &str) -> Result<()> {
            self.recorded.borrow_mut().push(session.to_string());
            if self.fail {
                Err(ZError::Io("record failed".to_string()))
            } else {
                Ok(())
            }
        }

        fn load_activity(&self) -> SessionActivity {
            SessionActivity::new()
        }

        fn remove_entry(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn record_session_attach_records_activity() {
        let activity = FakeActivity::ok();

        let effects = record_session_attach(&activity, "app:main");

        assert_eq!(effects.activity_recorded, true);
        assert_eq!(activity.recorded.borrow().as_slice(), ["app:main"]);
    }

    #[test]
    fn record_session_attach_best_effort() {
        let activity = FakeActivity::failing();

        let effects = record_session_attach(&activity, "app:main");

        assert_eq!(effects.activity_recorded, false);
    }
}
