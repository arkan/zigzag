use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use z_core::error::{Result, ZError};
use z_core::log::{LogEntry, LogLevel, Logger};

const MAX_LOG_SIZE: u64 = 1_048_576; // 1 MB
const TRUNCATE_TO: usize = 524_288; // 500 KB

/// File-based logger that appends to `~/.local/state/z/z.log`.
pub struct FileLogger {
    path: PathBuf,
}

impl FileLogger {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let path = PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("z")
            .join("z.log");
        Self { path }
    }

    /// Read the last `max_lines` entries from the log file.
    pub fn read_recent(&self, max_lines: usize) -> Vec<LogEntry> {
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        content
            .lines()
            .rev()
            .take(max_lines)
            .filter_map(LogEntry::parse)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Read the entire log file as a string.
    pub fn read_all(&self) -> Result<String> {
        fs::read_to_string(&self.path)
            .map_err(|e| ZError::Io(format!("failed to read log file: {}", e)))
    }

    /// Truncate if file exceeds MAX_LOG_SIZE, keeping the most recent entries.
    fn maybe_rotate(&self) {
        let metadata = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return,
        };
        if metadata.len() <= MAX_LOG_SIZE {
            return;
        }
        let content = match fs::read(&self.path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let start = content.len().saturating_sub(TRUNCATE_TO);
        // Find the first newline after `start` to avoid partial lines.
        let actual_start = content[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p + 1)
            .unwrap_or(start);
        let _ = fs::write(&self.path, &content[actual_start..]);
    }
}

/// Format current time as `YYYY-MM-DDTHH:MM:SSZ`.
fn format_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut days = secs / 86400;

    // Calculate year/month/day from days since epoch.
    let mut year = 1970i64;
    loop {
        let days_in_year: u64 = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if days < md {
            month = i;
            break;
        }
        days -= md;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month + 1,
        days + 1,
        h,
        m,
        s
    )
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

impl Logger for FileLogger {
    fn log(&self, level: LogLevel, message: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ZError::Io(format!("failed to create log dir: {}", e)))?;
        }

        let entry = LogEntry {
            timestamp: format_timestamp(),
            level,
            message: message.to_string(),
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| ZError::Io(format!("failed to open log file: {}", e)))?;

        writeln!(file, "{}", entry.format())
            .map_err(|e| ZError::Io(format!("failed to write log: {}", e)))?;

        self.maybe_rotate();
        Ok(())
    }
}

/// Convenience: log a message at Info level (best-effort, never panics).
pub fn log_info(logger: &FileLogger, msg: &str) {
    let _ = logger.log(LogLevel::Info, msg);
}

/// Convenience: log a message at Error level (best-effort, never panics).
pub fn log_error(logger: &FileLogger, msg: &str) {
    let _ = logger.log(LogLevel::Error, msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_logger(name: &str) -> FileLogger {
        let path = PathBuf::from(format!(
            "/tmp/z-test-log-{}-{}.log",
            std::process::id(),
            name
        ));
        let _ = fs::remove_file(&path);
        FileLogger { path }
    }

    #[test]
    fn log_creates_file_and_appends() {
        let logger = temp_logger("create");
        logger.log(LogLevel::Info, "hello").unwrap();
        logger.log(LogLevel::Error, "world").unwrap();

        let content = fs::read_to_string(&logger.path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[INFO] hello"));
        assert!(lines[1].contains("[ERROR] world"));
        let _ = fs::remove_file(&logger.path);
    }

    #[test]
    fn read_recent_returns_last_n() {
        let logger = temp_logger("recent");
        for i in 0..10 {
            logger.log(LogLevel::Info, &format!("msg-{}", i)).unwrap();
        }
        let recent = logger.read_recent(3);
        assert_eq!(recent.len(), 3);
        assert!(recent[0].message.contains("msg-7"));
        assert!(recent[1].message.contains("msg-8"));
        assert!(recent[2].message.contains("msg-9"));
        let _ = fs::remove_file(&logger.path);
    }

    #[test]
    fn read_recent_empty_file() {
        let logger = temp_logger("empty");
        let recent = logger.read_recent(10);
        assert!(recent.is_empty());
    }

    #[test]
    fn format_timestamp_is_valid() {
        let ts = format_timestamp();
        assert!(ts.starts_with("20"));
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 20);
    }

    #[test]
    fn log_info_convenience_does_not_panic() {
        let logger = temp_logger("info");
        log_info(&logger, "test message");
        let content = fs::read_to_string(&logger.path).unwrap();
        assert!(content.contains("[INFO] test message"));
        let _ = fs::remove_file(&logger.path);
    }

    #[test]
    fn log_error_convenience_does_not_panic() {
        let logger = temp_logger("error");
        log_error(&logger, "bad thing");
        let content = fs::read_to_string(&logger.path).unwrap();
        assert!(content.contains("[ERROR] bad thing"));
        let _ = fs::remove_file(&logger.path);
    }
}
