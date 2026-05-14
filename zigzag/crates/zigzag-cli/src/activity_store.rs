use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use zigzag_core::activity::{ActivityStore, SessionActivity};
use zigzag_core::error::{Result, ZError};

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// File-backed Adapter for persisted Session activity.
#[derive(Debug, Clone)]
pub struct FileActivityStore {
    path: PathBuf,
}

impl FileActivityStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config/z/session-activity.json")
    }

    pub fn record_attach(&self, session: &str) -> Result<()> {
        self.record_attach_at(session, unix_now_secs())
    }

    pub fn record_attach_at(&self, session: &str, now_secs: u64) -> Result<()> {
        let mut activity = self.load_activity();
        activity.insert(session.to_string(), now_secs);
        self.write(&activity)
    }

    pub fn load_activity(&self) -> SessionActivity {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(_) => return SessionActivity::new(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn remove_entry(&self, session: &str) -> Result<()> {
        let mut activity = self.load_activity();
        if activity.remove(session).is_none() {
            return Ok(());
        }
        self.write(&activity)
    }

    fn write(&self, activity: &SessionActivity) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| ZError::Io(e.to_string()))?;
        }
        let bytes = serde_json::to_vec(activity)
            .map_err(|e| ZError::Io(format!("serialize activity: {}", e)))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes).map_err(|e| ZError::Io(e.to_string()))?;
        fs::rename(&tmp, &self.path).map_err(|e| ZError::Io(e.to_string()))?;
        Ok(())
    }
}

impl Default for FileActivityStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

impl ActivityStore for FileActivityStore {
    fn record_attach(&self, session: &str) -> Result<()> {
        FileActivityStore::record_attach(self, session)
    }

    fn load_activity(&self) -> SessionActivity {
        FileActivityStore::load_activity(self)
    }

    fn remove_entry(&self, session: &str) -> Result<()> {
        FileActivityStore::remove_entry(self, session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_activity_file() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("z-activity-test-{}-{}.json", pid, n))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn record_then_load_returns_session_timestamp() {
        let path = tmp_activity_file();
        let store = FileActivityStore::new(path.clone());
        store.record_attach_at("myapp:main", 1_700_000_000).unwrap();
        let activity = store.load_activity();
        assert_eq!(activity.get("myapp:main"), Some(&1_700_000_000));
        cleanup(&path);
    }

    #[test]
    fn recording_one_session_preserves_others() {
        let path = tmp_activity_file();
        let store = FileActivityStore::new(path.clone());
        store.record_attach_at("myapp:main", 1_700_000_000).unwrap();
        store.record_attach_at("other:dev", 1_800_000_000).unwrap();
        let activity = store.load_activity();
        assert_eq!(activity.get("myapp:main"), Some(&1_700_000_000));
        assert_eq!(activity.get("other:dev"), Some(&1_800_000_000));
        cleanup(&path);
    }

    #[test]
    fn remove_entry_deletes_one_keeps_others() {
        let path = tmp_activity_file();
        let store = FileActivityStore::new(path.clone());
        store.record_attach_at("myapp:main", 1_700_000_000).unwrap();
        store.record_attach_at("other:dev", 1_800_000_000).unwrap();
        store.remove_entry("myapp:main").unwrap();
        let activity = store.load_activity();
        assert!(!activity.contains_key("myapp:main"));
        assert_eq!(activity.get("other:dev"), Some(&1_800_000_000));
        cleanup(&path);
    }

    #[test]
    fn remove_missing_entry_is_noop() {
        let path = tmp_activity_file();
        let store = FileActivityStore::new(path.clone());
        store.remove_entry("not:there").unwrap();
        let activity = store.load_activity();
        assert!(activity.is_empty());
        cleanup(&path);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = tmp_activity_file();
        cleanup(&path);
        let store = FileActivityStore::new(path);
        let activity = store.load_activity();
        assert!(activity.is_empty());
    }

    #[test]
    fn load_corrupt_file_returns_empty() {
        let path = tmp_activity_file();
        std::fs::write(&path, b"not json").unwrap();
        let store = FileActivityStore::new(path.clone());
        let activity = store.load_activity();
        assert!(activity.is_empty());
        cleanup(&path);
    }

    #[test]
    fn record_twice_for_same_session_keeps_latest() {
        let path = tmp_activity_file();
        let store = FileActivityStore::new(path.clone());
        store.record_attach_at("myapp:main", 1_700_000_000).unwrap();
        store.record_attach_at("myapp:main", 1_800_000_000).unwrap();
        let activity = store.load_activity();
        assert_eq!(activity.get("myapp:main"), Some(&1_800_000_000));
        cleanup(&path);
    }
}
