use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use z_core::domain::NotifyLevel;
use z_core::error::{Result, ZError};
use z_core::notification::{
    format_notification_content, validate_session_name, NotificationStore,
};

static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

/// File-backed Adapter for pending Session notifications.
#[derive(Debug, Clone)]
pub struct FileNotificationStore {
    base_dir: PathBuf,
}

impl FileNotificationStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn default_dir() -> PathBuf {
        PathBuf::from("/tmp/z/notifications")
    }

    pub fn session_dir(&self, session: &str) -> PathBuf {
        self.base_dir.join(session)
    }

    pub fn write_notification(&self, session: &str, message: &str, level: NotifyLevel) -> Result<()> {
        validate_session_name(session)?;
        let dir = self.session_dir(session);
        fs::create_dir_all(&dir).map_err(|e| ZError::Io(e.to_string()))?;

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{}_{}", ts, seq));
        fs::write(&path, format_notification_content(message, level))
            .map_err(|e| ZError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn clear_notifications(&self, session: &str) -> Result<()> {
        validate_session_name(session)?;
        let dir = self.session_dir(session);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| ZError::Io(e.to_string()))?;
        }
        Ok(())
    }

    pub fn has_notifications(&self, session: &str) -> bool {
        let dir = self.session_dir(session);
        dir.exists()
            && fs::read_dir(&dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
    }

    pub fn count_notifications(&self, session: &str) -> usize {
        let dir = self.session_dir(session);
        if !dir.exists() {
            return 0;
        }
        fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0)
    }

    pub fn sessions_with_notifications(&self) -> Vec<String> {
        if !self.base_dir.exists() {
            return Vec::new();
        }
        fs::read_dir(&self.base_dir)
            .map(|entries| {
                entries
                    .filter_map(|res| {
                        let entry = res.ok()?;
                        let path = entry.path();
                        if !path.is_dir() {
                            return None;
                        }
                        let non_empty = fs::read_dir(&path)
                            .map(|mut d| d.next().is_some())
                            .unwrap_or(false);
                        if non_empty {
                            Some(entry.file_name().to_string_lossy().to_string())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for FileNotificationStore {
    fn default() -> Self {
        Self::new(Self::default_dir())
    }
}

impl NotificationStore for FileNotificationStore {
    fn write_notification(&self, session: &str, message: &str, level: NotifyLevel) -> Result<()> {
        FileNotificationStore::write_notification(self, session, message, level)
    }

    fn clear_notifications(&self, session: &str) -> Result<()> {
        FileNotificationStore::clear_notifications(self, session)
    }

    fn has_notifications(&self, session: &str) -> bool {
        FileNotificationStore::has_notifications(self, session)
    }

    fn count_notifications(&self, session: &str) -> usize {
        FileNotificationStore::count_notifications(self, session)
    }

    fn sessions_with_notifications(&self) -> Vec<String> {
        FileNotificationStore::sessions_with_notifications(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (FileNotificationStore, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("z-notification-test-{}-{}", pid, n));
        let _ = std::fs::remove_dir_all(&dir);
        (FileNotificationStore::new(dir.clone()), dir)
    }

    fn cleanup(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_creates_notification_file() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "hello", NotifyLevel::Info).unwrap();
        assert!(store.has_notifications("myapp:main"));
        cleanup(&dir);
    }

    #[test]
    fn has_notifications_false_when_no_files() {
        let (store, dir) = temp_store();
        assert!(!store.has_notifications("myapp:main"));
        cleanup(&dir);
    }

    #[test]
    fn clear_notifications_removes_files() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "msg", NotifyLevel::Warning).unwrap();
        assert!(store.has_notifications("myapp:main"));
        store.clear_notifications("myapp:main").unwrap();
        assert!(!store.has_notifications("myapp:main"));
        cleanup(&dir);
    }

    #[test]
    fn clear_notifications_is_idempotent() {
        let (store, dir) = temp_store();
        store.clear_notifications("myapp:main").unwrap();
        store.clear_notifications("myapp:main").unwrap();
        cleanup(&dir);
    }

    #[test]
    fn sessions_with_notifications_includes_session_with_file() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "test", NotifyLevel::Error).unwrap();
        let sessions = store.sessions_with_notifications();
        assert!(sessions.contains(&"myapp:main".to_string()));
        cleanup(&dir);
    }

    #[test]
    fn sessions_with_notifications_excludes_cleared_session() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "test", NotifyLevel::Info).unwrap();
        store.clear_notifications("myapp:main").unwrap();
        let sessions = store.sessions_with_notifications();
        assert!(!sessions.contains(&"myapp:main".to_string()));
        cleanup(&dir);
    }

    #[test]
    fn write_multiple_notifications_all_present() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "first", NotifyLevel::Info).unwrap();
        store.write_notification("myapp:main", "second", NotifyLevel::Warning).unwrap();
        assert_eq!(store.count_notifications("myapp:main"), 2);
        cleanup(&dir);
    }

    #[test]
    fn count_notifications_zero_when_dir_does_not_exist() {
        let (store, dir) = temp_store();
        assert_eq!(store.count_notifications("nonexistent"), 0);
        cleanup(&dir);
    }

    #[test]
    fn count_notifications_counts_files() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "first", NotifyLevel::Info).unwrap();
        store.write_notification("myapp:main", "second", NotifyLevel::Warning).unwrap();
        store.write_notification("myapp:main", "third", NotifyLevel::Error).unwrap();
        assert_eq!(store.count_notifications("myapp:main"), 3);
        cleanup(&dir);
    }

    #[test]
    fn count_notifications_zero_after_clear() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "msg", NotifyLevel::Info).unwrap();
        assert_eq!(store.count_notifications("myapp:main"), 1);
        store.clear_notifications("myapp:main").unwrap();
        assert_eq!(store.count_notifications("myapp:main"), 0);
        cleanup(&dir);
    }

    #[test]
    fn write_rejects_invalid_session_names() {
        let (store, dir) = temp_store();
        assert!(store.write_notification("../escape", "msg", NotifyLevel::Info).is_err());
        assert!(store.write_notification("foo/bar", "msg", NotifyLevel::Info).is_err());
        assert!(store.write_notification("", "msg", NotifyLevel::Info).is_err());
        assert!(store.write_notification(".", "msg", NotifyLevel::Info).is_err());
        cleanup(&dir);
    }

    #[test]
    fn clear_rejects_traversal_session() {
        let (store, dir) = temp_store();
        assert!(store.clear_notifications("../../etc").is_err());
        cleanup(&dir);
    }

    #[test]
    fn notification_file_contains_level_and_message() {
        let (store, dir) = temp_store();
        store.write_notification("myapp:main", "deployment done", NotifyLevel::Info).unwrap();
        let file = fs::read_dir(store.session_dir("myapp:main"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let content = fs::read_to_string(&file).unwrap();
        assert!(content.starts_with("info\n"));
        assert!(content.contains("deployment done"));
        cleanup(&dir);
    }
}
