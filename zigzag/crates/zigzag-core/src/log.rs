use crate::error::Result;
use std::fmt;

/// Log severity level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARNING",
            LogLevel::Error => "ERROR",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "INFO" => Some(LogLevel::Info),
            "WARNING" => Some(LogLevel::Warning),
            "ERROR" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single log entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    /// Format to the canonical line: `[timestamp] [LEVEL] message`
    pub fn format(&self) -> String {
        format!("[{}] [{}] {}", self.timestamp, self.level, self.message)
    }

    /// Parse a log line like `[2026-04-06T14:23:01Z] [INFO] message here`.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if !line.starts_with('[') {
            return None;
        }
        let ts_end = line.find(']')?;
        let timestamp = line[1..ts_end].to_string();

        let rest = &line[ts_end + 1..].trim_start();
        if !rest.starts_with('[') {
            return None;
        }
        let level_end = rest.find(']')?;
        let level = LogLevel::parse(&rest[1..level_end])?;

        let message = rest[level_end + 1..].trim_start().to_string();

        Some(LogEntry {
            timestamp,
            level,
            message,
        })
    }
}

/// Trait for appending log entries. Implementations live in zigzag-cli.
pub trait Logger {
    fn log(&self, level: LogLevel, message: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_info_entry() {
        let entry = LogEntry {
            timestamp: "2026-04-06T14:23:01Z".to_string(),
            level: LogLevel::Info,
            message: "session myapp:main created".to_string(),
        };
        assert_eq!(
            entry.format(),
            "[2026-04-06T14:23:01Z] [INFO] session myapp:main created"
        );
    }

    #[test]
    fn parse_roundtrip() {
        let line = "[2026-04-06T14:23:01Z] [WARNING] worktree skipped: uncommitted changes";
        let entry = LogEntry::parse(line).expect("should parse");
        assert_eq!(entry.timestamp, "2026-04-06T14:23:01Z");
        assert_eq!(entry.level, LogLevel::Warning);
        assert_eq!(entry.message, "worktree skipped: uncommitted changes");
        assert_eq!(entry.format(), line);
    }

    #[test]
    fn parse_error_level() {
        let line = "[2026-04-06T15:00:00Z] [ERROR] session kill failed";
        let entry = LogEntry::parse(line).expect("should parse");
        assert_eq!(entry.level, LogLevel::Error);
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(LogEntry::parse("").is_none());
        assert!(LogEntry::parse("no brackets").is_none());
        assert!(LogEntry::parse("[ts] no level").is_none());
        assert!(LogEntry::parse("[ts] [UNKNOWN] msg").is_none());
    }

    #[test]
    fn display_levels() {
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Warning.to_string(), "WARNING");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
    }
}
