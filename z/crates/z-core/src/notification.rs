/// File-based notification storage for z.
///
/// Events are written to `/tmp/z/notifications/{session}/{timestamp_ns}`.
/// The TUI reads this directory to display 🔔 badges on sessions with
/// pending notifications. Notifications are cleared when the user opens
/// the session.
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::NotifyLevel;
use crate::error::{Result, ZError};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Base directory for all session notification files.
pub fn notifications_dir() -> PathBuf {
    PathBuf::from("/tmp/z/notifications")
}

/// Per-session notification directory: `/tmp/z/notifications/{session}`.
pub fn session_notifications_dir(session: &str) -> PathBuf {
    notifications_dir().join(session)
}

// ---------------------------------------------------------------------------
// Write / clear
// ---------------------------------------------------------------------------

/// Write a notification event file for `session`.
///
/// Creates `/tmp/z/notifications/{session}/{timestamp_ns}` with the format:
/// ```text
/// <level>
/// <message>
/// ```
pub fn write_notification(session: &str, message: &str, level: NotifyLevel) -> Result<()> {
    let dir = session_notifications_dir(session);
    fs::create_dir_all(&dir).map_err(|e| ZError::Io(e.to_string()))?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let level_str = match level {
        NotifyLevel::Info => "info",
        NotifyLevel::Warning => "warning",
        NotifyLevel::Error => "error",
    };

    let content = format!("{}\n{}", level_str, message);
    let path = dir.join(format!("{}", ts));
    fs::write(&path, content).map_err(|e| ZError::Io(e.to_string()))?;
    Ok(())
}

/// Remove all notification files for `session`.
pub fn clear_notifications(session: &str) -> Result<()> {
    let dir = session_notifications_dir(session);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| ZError::Io(e.to_string()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// Returns `true` if there is at least one pending notification for `session`.
pub fn has_notifications(session: &str) -> bool {
    let dir = session_notifications_dir(session);
    dir.exists()
        && fs::read_dir(&dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

/// Returns the names of all sessions that have pending notifications.
///
/// Scans subdirectories of `/tmp/z/notifications/`; only includes directories
/// that contain at least one file.
pub fn sessions_with_notifications() -> Vec<String> {
    let base = notifications_dir();
    if !base.exists() {
        return Vec::new();
    }
    fs::read_dir(&base)
        .map(|entries| {
            entries
                .filter_map(|res| {
                    let entry = res.ok()?;
                    let path = entry.path();
                    if !path.is_dir() {
                        return None;
                    }
                    // Only include if the directory has at least one file.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Use a unique prefix per test to avoid collisions when tests run in parallel.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_session(prefix: &str) -> String {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}__test__{}", prefix, n)
    }

    fn cleanup(session: &str) {
        let _ = clear_notifications(session);
    }

    #[test]
    fn write_creates_notification_file() {
        let session = unique_session("write_creates");
        cleanup(&session);

        write_notification(&session, "hello", NotifyLevel::Info).unwrap();
        assert!(has_notifications(&session));
        cleanup(&session);
    }

    #[test]
    fn has_notifications_false_when_no_files() {
        let session = unique_session("has_no_files");
        cleanup(&session); // ensure clean state
        assert!(!has_notifications(&session));
    }

    #[test]
    fn clear_notifications_removes_files() {
        let session = unique_session("clear_removes");
        cleanup(&session);

        write_notification(&session, "msg", NotifyLevel::Warning).unwrap();
        assert!(has_notifications(&session));

        clear_notifications(&session).unwrap();
        assert!(!has_notifications(&session));
    }

    #[test]
    fn clear_notifications_is_idempotent() {
        let session = unique_session("clear_idempotent");
        cleanup(&session);
        // Clearing a non-existent session should not fail.
        clear_notifications(&session).unwrap();
        clear_notifications(&session).unwrap();
    }

    #[test]
    fn sessions_with_notifications_includes_session_with_file() {
        let session = unique_session("list_includes");
        cleanup(&session);

        write_notification(&session, "test", NotifyLevel::Error).unwrap();
        let sessions = sessions_with_notifications();
        assert!(
            sessions.contains(&session),
            "should list session with pending notification; got {:?}",
            sessions
        );
        cleanup(&session);
    }

    #[test]
    fn sessions_with_notifications_excludes_cleared_session() {
        let session = unique_session("list_excludes_cleared");
        cleanup(&session);

        write_notification(&session, "test", NotifyLevel::Info).unwrap();
        clear_notifications(&session).unwrap();
        let sessions = sessions_with_notifications();
        assert!(
            !sessions.contains(&session),
            "cleared session should not appear in list; got {:?}",
            sessions
        );
    }

    #[test]
    fn write_multiple_notifications_all_present() {
        let session = unique_session("multi_write");
        cleanup(&session);

        write_notification(&session, "first", NotifyLevel::Info).unwrap();
        // Small sleep not needed — timestamps use nanoseconds and writes are sequential.
        write_notification(&session, "second", NotifyLevel::Warning).unwrap();

        let dir = session_notifications_dir(&session);
        let count = fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 2, "expected 2 notification files");
        cleanup(&session);
    }

    #[test]
    fn notification_file_contains_level_and_message() {
        let session = unique_session("file_content");
        cleanup(&session);

        write_notification(&session, "deployment done", NotifyLevel::Info).unwrap();

        let dir = session_notifications_dir(&session);
        let file = fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let content = fs::read_to_string(&file).unwrap();
        assert!(content.starts_with("info\n"), "level line missing: {:?}", content);
        assert!(content.contains("deployment done"), "message missing: {:?}", content);
        cleanup(&session);
    }
}
