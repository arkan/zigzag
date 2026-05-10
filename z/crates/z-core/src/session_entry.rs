use crate::activity::ActivityStore;
use crate::notification::NotificationStore;

/// Best-effort effects applied when entering a Session.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionEntryEffects {
    pub notifications_cleared: bool,
    pub activity_recorded: bool,
}

/// Clear pending notifications for a Session.
pub fn clear_session_notifications(
    notifications: &dyn NotificationStore,
    session: &str,
) -> SessionEntryEffects {
    SessionEntryEffects {
        notifications_cleared: notifications.clear_notifications(session).is_ok(),
        activity_recorded: false,
    }
}

/// Record that a Session was attached/opened.
pub fn record_session_attach(activity: &dyn ActivityStore, session: &str) -> SessionEntryEffects {
    SessionEntryEffects {
        notifications_cleared: false,
        activity_recorded: activity.record_attach(session).is_ok(),
    }
}

/// Mark an existing Session as entered: clear notifications and record activity.
pub fn mark_existing_session_entered(
    notifications: &dyn NotificationStore,
    activity: &dyn ActivityStore,
    session: &str,
) -> SessionEntryEffects {
    let notifications_cleared = notifications.clear_notifications(session).is_ok();
    let activity_recorded = activity.record_attach(session).is_ok();
    SessionEntryEffects {
        notifications_cleared,
        activity_recorded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::SessionActivity;
    use crate::domain::NotifyLevel;
    use crate::error::{Result, ZError};
    use std::cell::RefCell;

    struct FakeNotifications {
        cleared: RefCell<Vec<String>>,
        fail: bool,
    }

    impl FakeNotifications {
        fn ok() -> Self {
            Self { cleared: RefCell::new(Vec::new()), fail: false }
        }

        fn failing() -> Self {
            Self { cleared: RefCell::new(Vec::new()), fail: true }
        }
    }

    impl NotificationStore for FakeNotifications {
        fn write_notification(&self, _: &str, _: &str, _: NotifyLevel) -> Result<()> {
            Ok(())
        }

        fn clear_notifications(&self, session: &str) -> Result<()> {
            self.cleared.borrow_mut().push(session.to_string());
            if self.fail {
                Err(ZError::Io("clear failed".to_string()))
            } else {
                Ok(())
            }
        }

        fn has_notifications(&self, _: &str) -> bool {
            false
        }

        fn count_notifications(&self, _: &str) -> usize {
            0
        }

        fn sessions_with_notifications(&self) -> Vec<String> {
            Vec::new()
        }
    }

    struct FakeActivity {
        recorded: RefCell<Vec<String>>,
        fail: bool,
    }

    impl FakeActivity {
        fn ok() -> Self {
            Self { recorded: RefCell::new(Vec::new()), fail: false }
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
    fn mark_existing_session_clears_notifications_and_records_activity() {
        let notifications = FakeNotifications::ok();
        let activity = FakeActivity::ok();

        let effects = mark_existing_session_entered(&notifications, &activity, "app:main");

        assert_eq!(effects.notifications_cleared, true);
        assert_eq!(effects.activity_recorded, true);
        assert_eq!(notifications.cleared.borrow().as_slice(), ["app:main"]);
        assert_eq!(activity.recorded.borrow().as_slice(), ["app:main"]);
    }

    #[test]
    fn entry_effects_are_best_effort() {
        let notifications = FakeNotifications::failing();
        let activity = FakeActivity::ok();

        let effects = mark_existing_session_entered(&notifications, &activity, "app:main");

        assert_eq!(effects.notifications_cleared, false);
        assert_eq!(effects.activity_recorded, true);
    }
}
