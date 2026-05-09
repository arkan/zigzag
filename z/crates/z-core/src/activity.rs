use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::Session;
use crate::error::{Result, ZError};

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record that `session` was attached at the current time. Failures to write
/// the activity file are ignored (best-effort tracking).
pub fn record_attach(session: &str) {
    let _ = ActivityLog::default().record_attach(session);
}

/// Load the activity map from the default location.
pub fn load_activity() -> HashMap<String, u64> {
    ActivityLog::default().load()
}

/// Remove `session` from the activity file. Failures are ignored.
pub fn remove_entry(session: &str) {
    let _ = ActivityLog::default().remove_entry(session);
}

/// Path of the session activity file (`~/.config/z/session-activity.json`).
pub fn activity_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/z/session-activity.json")
}

/// Path-scoped Adapter for persisted Session activity.
///
/// The public free functions above keep the existing Interface stable, while
/// this Module concentrates the filesystem Implementation behind one testable
/// seam.
#[derive(Debug, Clone)]
pub struct ActivityLog {
    path: PathBuf,
}

impl ActivityLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn record_attach(&self, session: &str) -> Result<()> {
        self.record_attach_at(session, unix_now_secs())
    }

    pub fn record_attach_at(&self, session: &str, now_secs: u64) -> Result<()> {
        let mut activity = self.load();
        activity.insert(session.to_string(), now_secs);
        self.write(&activity)
    }

    /// Load the activity map. Returns an empty map if the file is missing or malformed.
    pub fn load(&self) -> HashMap<String, u64> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(_) => return HashMap::new(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Remove `session` from the activity file. No-op if the file or entry is missing.
    pub fn remove_entry(&self, session: &str) -> Result<()> {
        let mut activity = self.load();
        if activity.remove(session).is_none() {
            return Ok(());
        }
        self.write(&activity)
    }

    fn write(&self, activity: &HashMap<String, u64>) -> Result<()> {
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

impl Default for ActivityLog {
    fn default() -> Self {
        Self::new(activity_file_path())
    }
}

/// Record that `session` was attached at `now_secs`.
#[cfg(test)]
pub(crate) fn record_attach_at(path: &std::path::Path, session: &str, now_secs: u64) -> Result<()> {
    ActivityLog::new(path).record_attach_at(session, now_secs)
}

/// Load the activity map from `path`. Returns empty map if file is missing
/// or malformed.
#[cfg(test)]
pub(crate) fn load_activity_from(path: &std::path::Path) -> HashMap<String, u64> {
    ActivityLog::new(path).load()
}

/// Remove `session` from the activity file. No-op if the file or entry is
/// missing.
#[cfg(test)]
pub(crate) fn remove_entry_at(path: &std::path::Path, session: &str) -> Result<()> {
    ActivityLog::new(path).remove_entry(session)
}

/// Sort `sessions` in-place by their last-attach timestamp, most recent first.
/// Sessions without a recorded timestamp are placed at the end in their
/// original relative order (stable sort).
pub fn sort_sessions_by_recent_attach(sessions: &mut [Session], activity: &HashMap<String, u64>) {
    sort_by_recent_attach(sessions, activity, |s| &s.name);
}

/// Stable-sort `items` in-place by last-attach timestamp, most recent first.
/// Items whose session name has no recorded timestamp are placed at the end
/// in their original relative order. `name_of` extracts the session name
/// (format `project:branch`) from each item.
pub fn sort_by_recent_attach<T, F>(items: &mut [T], activity: &HashMap<String, u64>, name_of: F)
where
    F: Fn(&T) -> &str,
{
    items.sort_by(|a, b| {
        let ta = activity.get(name_of(a)).copied().unwrap_or(0);
        let tb = activity.get(name_of(b)).copied().unwrap_or(0);
        tb.cmp(&ta)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_activity_file() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("z-activity-test-{}-{}.json", pid, n))
    }

    #[test]
    fn record_then_load_returns_session_timestamp() {
        let path = tmp_activity_file();
        record_attach_at(&path, "myapp:main", 1_700_000_000).unwrap();
        let activity = load_activity_from(&path);
        assert_eq!(activity.get("myapp:main"), Some(&1_700_000_000));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn activity_log_uses_configured_path() {
        let path = tmp_activity_file();
        let log = ActivityLog::new(path.clone());
        log.record_attach_at("myapp:main", 1_700_000_000).unwrap();
        log.record_attach_at("other:dev", 1_800_000_000).unwrap();
        log.remove_entry("myapp:main").unwrap();

        let activity = log.load();
        assert!(!activity.contains_key("myapp:main"));
        assert_eq!(activity.get("other:dev"), Some(&1_800_000_000));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recording_one_session_preserves_others() {
        let path = tmp_activity_file();
        record_attach_at(&path, "myapp:main", 1_700_000_000).unwrap();
        record_attach_at(&path, "other:dev", 1_800_000_000).unwrap();
        let activity = load_activity_from(&path);
        assert_eq!(activity.get("myapp:main"), Some(&1_700_000_000));
        assert_eq!(activity.get("other:dev"), Some(&1_800_000_000));
        let _ = std::fs::remove_file(&path);
    }

    fn sess(name: &str) -> Session {
        let (project, branch) = name.split_once(':').unwrap();
        Session::new(project, branch)
    }

    #[test]
    fn sort_puts_most_recent_attach_first() {
        let mut sessions = vec![sess("app:old"), sess("app:new"), sess("app:mid")];
        let activity: HashMap<String, u64> = [
            ("app:old".to_string(), 100),
            ("app:new".to_string(), 300),
            ("app:mid".to_string(), 200),
        ]
        .into_iter()
        .collect();
        sort_sessions_by_recent_attach(&mut sessions, &activity);
        assert_eq!(
            sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["app:new", "app:mid", "app:old"]
        );
    }

    #[test]
    fn sort_by_recent_attach_works_on_string_tuples() {
        let mut items: Vec<(String, u32)> = vec![
            ("app:a".to_string(), 1),
            ("app:b".to_string(), 2),
            ("app:c".to_string(), 3),
        ];
        let activity: HashMap<String, u64> = [
            ("app:a".to_string(), 100),
            ("app:b".to_string(), 500),
            ("app:c".to_string(), 250),
        ]
        .into_iter()
        .collect();
        sort_by_recent_attach(&mut items, &activity, |it| it.0.as_str());
        assert_eq!(
            items.iter().map(|it| it.0.as_str()).collect::<Vec<_>>(),
            vec!["app:b", "app:c", "app:a"]
        );
    }

    #[test]
    fn sort_places_sessions_without_activity_at_end() {
        let mut sessions = vec![sess("app:unknown"), sess("app:recent"), sess("app:ancient")];
        let activity: HashMap<String, u64> = [
            ("app:recent".to_string(), 500),
            ("app:ancient".to_string(), 10),
        ]
        .into_iter()
        .collect();
        sort_sessions_by_recent_attach(&mut sessions, &activity);
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["app:recent", "app:ancient", "app:unknown"]);
    }

    #[test]
    fn remove_entry_deletes_one_keeps_others() {
        let path = tmp_activity_file();
        record_attach_at(&path, "myapp:main", 1_700_000_000).unwrap();
        record_attach_at(&path, "other:dev", 1_800_000_000).unwrap();
        remove_entry_at(&path, "myapp:main").unwrap();
        let activity = load_activity_from(&path);
        assert!(!activity.contains_key("myapp:main"));
        assert_eq!(activity.get("other:dev"), Some(&1_800_000_000));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_missing_entry_is_noop() {
        let path = tmp_activity_file();
        remove_entry_at(&path, "not:there").unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = tmp_activity_file();
        let activity = load_activity_from(&path);
        assert!(activity.is_empty());
    }

    #[test]
    fn load_corrupt_file_returns_empty() {
        let path = tmp_activity_file();
        std::fs::write(&path, b"not json at all {{{").unwrap();
        let activity = load_activity_from(&path);
        assert!(activity.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_twice_for_same_session_keeps_latest() {
        let path = tmp_activity_file();
        record_attach_at(&path, "myapp:main", 1_700_000_000).unwrap();
        record_attach_at(&path, "myapp:main", 1_800_000_000).unwrap();
        let activity = load_activity_from(&path);
        assert_eq!(activity.get("myapp:main"), Some(&1_800_000_000));
        assert_eq!(activity.len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
