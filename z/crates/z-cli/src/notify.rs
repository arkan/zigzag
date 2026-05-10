/// Notifier implementations for z-cli.
///
/// Provides concrete `Notifier` trait implementations:
/// - `MacosNotifier`  — sends a macOS native notification via `osascript`
/// - `TelegramNotifier` — sends a Telegram message via `curl`
/// - `DispatchNotifier` — fans out to all configured notifiers
use z_core::config::NotificationsConfig;
use z_core::domain::NotifyLevel;
use z_core::error::{Result, ZError};
use z_core::traits::Notifier;

// ---------------------------------------------------------------------------
// MacosNotifier
// ---------------------------------------------------------------------------

/// Sends a macOS native notification via `osascript`.
pub struct MacosNotifier;

impl Notifier for MacosNotifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
        let title = match level {
            NotifyLevel::Info => "z",
            NotifyLevel::Warning => "z \u{26a0}\u{fe0f}",
            NotifyLevel::Error => "z \u{274c}",
        };
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(message),
            applescript_quote(title),
        );
        let status = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|e| ZError::Io(format!("osascript: {}", e)))?;
        if !status.success() {
            return Err(ZError::Io(format!(
                "osascript exited with status {}",
                status
            )));
        }
        Ok(())
    }
}

/// Wrap a string in AppleScript double-quoted string literals.
///
/// AppleScript double-quoted strings only support `\\` and `\"` as escape
/// sequences — there is no `\n`. Newlines and carriage returns are replaced
/// with spaces to avoid syntax errors or injection.
fn applescript_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ")
            .replace('\r', " ")
    )
}

// ---------------------------------------------------------------------------
// TelegramNotifier
// ---------------------------------------------------------------------------

/// Sends a Telegram message via the Bot API using `curl`.
pub struct TelegramNotifier {
    pub token: String,
    pub chat_id: String,
}

impl Notifier for TelegramNotifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
        let prefix = match level {
            NotifyLevel::Info => "",
            NotifyLevel::Warning => "\u{26a0}\u{fe0f} ",
            NotifyLevel::Error => "\u{274c} ",
        };
        let text = format!("{}{}", prefix, message);
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.token
        );
        let body = format!(
            "chat_id={}&text={}",
            percent_encode(&self.chat_id),
            percent_encode(&text)
        );
        let status = std::process::Command::new("curl")
            .args([
                "-s",
                "-X", "POST",
                &url,
                "--data", &body,
            ])
            .status()
            .map_err(|e| ZError::Io(format!("curl (telegram): {}", e)))?;
        if !status.success() {
            return Err(ZError::Io(format!(
                "curl telegram request failed with status {}",
                status
            )));
        }
        Ok(())
    }
}

/// Minimal percent-encoding for `application/x-www-form-urlencoded` data.
fn percent_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            '&' => "%26".to_string(),
            '+' => "%2B".to_string(),
            '#' => "%23".to_string(),
            '=' => "%3D".to_string(),
            '\n' => "%0A".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// DispatchNotifier
// ---------------------------------------------------------------------------

/// Dispatches a notification to all configured channels.
///
/// Metadata-backed TUI badges are written by `cmd_notify`; this dispatcher only
/// fans out to external channels based on `config`.
///
/// If any notifier fails, the error is returned after attempting all of them
/// (best-effort delivery).
pub struct DispatchNotifier {
    notifiers: Vec<Box<dyn Notifier>>,
}

impl DispatchNotifier {
    pub fn from_config(config: &NotificationsConfig, _session: &str) -> Self {
        let mut notifiers: Vec<Box<dyn Notifier>> = Vec::new();

        if config.macos_native {
            notifiers.push(Box::new(MacosNotifier));
        }

        if config.telegram {
            if let (Some(token), Some(chat_id)) =
                (&config.telegram_token, &config.telegram_chat_id)
            {
                notifiers.push(Box::new(TelegramNotifier {
                    token: token.clone(),
                    chat_id: chat_id.clone(),
                }));
            }
        }

        Self { notifiers }
    }
}

impl Notifier for DispatchNotifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
        let mut last_err: Option<ZError> = None;
        for notifier in &self.notifiers {
            if let Err(e) = notifier.notify(message, level.clone()) {
                last_err = Some(e);
            }
        }
        if let Some(e) = last_err {
            return Err(e);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ── Mock notifier ─────────────────────────────────────────────────────

    struct MockNotifier {
        calls: Arc<Mutex<Vec<(String, NotifyLevel)>>>,
        fail: bool,
    }

    impl MockNotifier {
        fn new() -> (Self, Arc<Mutex<Vec<(String, NotifyLevel)>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let notifier = Self { calls: Arc::clone(&calls), fail: false };
            (notifier, calls)
        }

        fn failing() -> Self {
            let calls = Arc::new(Mutex::new(Vec::new()));
            Self { calls, fail: true }
        }
    }

    impl Notifier for MockNotifier {
        fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
            self.calls.lock().unwrap().push((message.to_string(), level));
            if self.fail {
                Err(ZError::Io("mock failure".to_string()))
            } else {
                Ok(())
            }
        }
    }

    // ── applescript_quote tests ───────────────────────────────────────────

    #[test]
    fn applescript_quote_plain_string() {
        assert_eq!(applescript_quote("hello"), "\"hello\"");
    }

    #[test]
    fn applescript_quote_escapes_double_quote() {
        assert_eq!(applescript_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn applescript_quote_escapes_backslash() {
        assert_eq!(applescript_quote("a\\b"), "\"a\\\\b\"");
    }

    // ── percent_encode tests ──────────────────────────────────────────────

    #[test]
    fn percent_encode_plain_string() {
        assert_eq!(percent_encode("hello"), "hello");
    }

    #[test]
    fn percent_encode_space() {
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn percent_encode_ampersand() {
        assert_eq!(percent_encode("a&b"), "a%26b");
    }

    #[test]
    fn percent_encode_equals() {
        assert_eq!(percent_encode("a=b"), "a%3Db");
    }

    #[test]
    fn percent_encode_newline() {
        assert_eq!(percent_encode("a\nb"), "a%0Ab");
    }

    // ── DispatchNotifier channel selection tests ──────────────────────────

    #[test]
    fn dispatch_does_not_write_legacy_file_notifications() {
        // Metadata-backed notifications are written by cmd_notify, not DispatchNotifier.
        // DispatchNotifier only fans out to external channels (macOS, Telegram).
        let config = NotificationsConfig {
            macos_native: false,
            telegram: false,
            tui: true,
            telegram_token: None,
            telegram_chat_id: None,
        };
        let dispatcher = DispatchNotifier::from_config(&config, "");
        dispatcher.notify("test msg", NotifyLevel::Info).unwrap();
        // No assertion needed beyond success — DispatchNotifier with empty
        // notifiers always succeeds and doesn't touch any file system.
    }

    #[test]
    fn dispatch_telegram_skipped_when_disabled() {
        // Telegram is configured but disabled — no external notifier runs.
        let config = NotificationsConfig {
            macos_native: false,
            telegram: false,
            tui: true,
            telegram_token: Some("fake_token".to_string()),
            telegram_chat_id: Some("123".to_string()),
        };
        let dispatcher = DispatchNotifier::from_config(&config, "");
        // Should succeed (no curl call attempted).
        dispatcher.notify("test", NotifyLevel::Info).unwrap();
    }

    #[test]
    fn dispatch_telegram_skipped_when_token_missing() {
        // Telegram enabled but no token configured — skipped gracefully.
        let config = NotificationsConfig {
            macos_native: false,
            telegram: true,
            tui: true,
            telegram_token: None,
            telegram_chat_id: Some("123".to_string()),
        };
        let dispatcher = DispatchNotifier::from_config(&config, "");
        dispatcher.notify("test", NotifyLevel::Info).unwrap();
    }

    // ── DispatchNotifier with mock notifiers ──────────────────────────────

    #[test]
    fn dispatch_notifier_calls_all_notifiers() {
        let (mock1, calls1) = MockNotifier::new();
        let (mock2, calls2) = MockNotifier::new();

        let dispatcher = DispatchNotifier {
            notifiers: vec![Box::new(mock1), Box::new(mock2)],
        };
        dispatcher.notify("hello", NotifyLevel::Warning).unwrap();

        let c1 = calls1.lock().unwrap();
        let c2 = calls2.lock().unwrap();
        assert_eq!(c1.len(), 1);
        assert_eq!(c2.len(), 1);
        assert_eq!(c1[0].0, "hello");
        assert_eq!(c1[0].1, NotifyLevel::Warning);
    }

    #[test]
    fn dispatch_continues_after_one_notifier_fails() {
        let (mock_ok, calls_ok) = MockNotifier::new();
        let mock_fail = MockNotifier::failing();

        let dispatcher = DispatchNotifier {
            notifiers: vec![Box::new(mock_fail), Box::new(mock_ok)],
        };
        // Should attempt both even though the first fails.
        let result = dispatcher.notify("msg", NotifyLevel::Error);
        assert!(result.is_err(), "should propagate the error");

        let c = calls_ok.lock().unwrap();
        assert_eq!(c.len(), 1, "second notifier should still have been called");
    }

    #[test]
    fn dispatch_empty_notifiers_succeeds() {
        let dispatcher = DispatchNotifier { notifiers: vec![] };
        dispatcher.notify("anything", NotifyLevel::Info).unwrap();
    }

    // ── percent_encode edge cases ─────────────────────────────────────────

    #[test]
    fn percent_encode_percent_sign() {
        assert_eq!(percent_encode("100%"), "100%25");
    }

    #[test]
    fn percent_encode_already_encoded_sequence() {
        // %26 in input must not be passed through as-is (would decode to &).
        assert_eq!(percent_encode("%26"), "%2526");
    }

    #[test]
    fn percent_encode_combined_specials() {
        assert_eq!(percent_encode("a=1&b=2"), "a%3D1%26b%3D2");
    }

    // ── applescript_quote edge cases ──────────────────────────────────────

    #[test]
    fn applescript_quote_strips_newlines() {
        assert_eq!(applescript_quote("line1\nline2"), "\"line1 line2\"");
    }

    #[test]
    fn applescript_quote_strips_carriage_return() {
        assert_eq!(applescript_quote("a\rb"), "\"a b\"");
    }

    #[test]
    fn applescript_quote_combined_escapes() {
        assert_eq!(
            applescript_quote("say \"hi\"\nand \\bye"),
            "\"say \\\"hi\\\" and \\\\bye\""
        );
    }

    // ── DispatchNotifier: telegram with missing chat_id ───────────────────

    #[test]
    fn dispatch_telegram_skipped_when_chat_id_missing() {
        let config = NotificationsConfig {
            macos_native: false,
            telegram: true,
            tui: true,
            telegram_token: Some("tok".to_string()),
            telegram_chat_id: None,
        };
        let dispatcher = DispatchNotifier::from_config(&config, "");
        // Should succeed — no curl call attempted.
        dispatcher.notify("test", NotifyLevel::Info).unwrap();
    }
}
