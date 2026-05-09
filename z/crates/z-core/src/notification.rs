use crate::domain::NotifyLevel;
use crate::error::{Result, ZError};

/// Storage Interface for pending Session notifications.
pub trait NotificationStore {
    fn write_notification(&self, session: &str, message: &str, level: NotifyLevel) -> Result<()>;
    fn clear_notifications(&self, session: &str) -> Result<()>;
    fn has_notifications(&self, session: &str) -> bool;
    fn count_notifications(&self, session: &str) -> usize;
    fn sessions_with_notifications(&self) -> Vec<String>;
}

/// Reject Session names that could escape an Adapter's storage root.
pub fn validate_session_name(session: &str) -> Result<()> {
    if session.is_empty()
        || session.contains('/')
        || session.contains('\\')
        || session.contains("..")
        || session == "."
    {
        return Err(ZError::Io(format!(
            "invalid session name for notifications: {:?}",
            session
        )));
    }
    Ok(())
}

/// Stable text representation written by file-backed notification Adapters.
pub fn format_notification_content(message: &str, level: NotifyLevel) -> String {
    format!("{}\n{}", level_name(level), message)
}

fn level_name(level: NotifyLevel) -> &'static str {
    match level {
        NotifyLevel::Info => "info",
        NotifyLevel::Warning => "warning",
        NotifyLevel::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_session_name_accepts_normal_session() {
        validate_session_name("myapp:main").unwrap();
    }

    #[test]
    fn validate_session_name_rejects_path_traversal() {
        assert!(validate_session_name("../escape").is_err());
        assert!(validate_session_name("foo/bar").is_err());
        assert!(validate_session_name("foo\\bar").is_err());
        assert!(validate_session_name("").is_err());
        assert!(validate_session_name(".").is_err());
    }

    #[test]
    fn format_notification_content_includes_level_and_message() {
        assert_eq!(
            format_notification_content("deployment done", NotifyLevel::Info),
            "info\ndeployment done"
        );
        assert_eq!(
            format_notification_content("careful", NotifyLevel::Warning),
            "warning\ncareful"
        );
        assert_eq!(
            format_notification_content("boom", NotifyLevel::Error),
            "error\nboom"
        );
    }
}
