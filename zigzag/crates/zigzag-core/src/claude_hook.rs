/// Merge logic for Claude Code `.claude/settings.json` Stop hooks.
///
/// Provides a pure function to inject a Z notification hook into an existing
/// (or absent) Claude Code settings file while preserving all other settings.
///
/// Claude Code hook format (v25.8+):
/// ```json
/// {
///   "hooks": {
///     "Stop": [
///       {
///         "matcher": "",
///         "hooks": [{ "type": "command", "command": "zigzag notify ..." }]
///       }
///     ]
///   }
/// }
/// ```
use serde_json::{json, Value};

const Z_HOOK_PREFIX: &str = "zigzag notify";

/// Returns true if a hook entry contains a Z notification command.
fn is_z_hook(entry: &Value) -> bool {
    // New format: entry.hooks[].command
    if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
        return hooks.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.starts_with(Z_HOOK_PREFIX))
        });
    }
    // Legacy format: entry.command
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.starts_with(Z_HOOK_PREFIX))
}

/// Merge a Z Stop hook into a Claude Code settings JSON value.
///
/// If `existing` is `None`, creates a fresh settings object.
/// Identifies Z hooks by command prefix `"zigzag notify"`.
pub fn merge_stop_hook(existing: Option<Value>, hook_command: &str) -> Value {
    let mut settings = existing.unwrap_or_else(|| json!({}));
    let hook_entry = json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": hook_command }]
    });

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();

    // Migrate from legacy "stop" key if present
    let legacy = hooks.remove("stop");

    let stop = hooks.entry("Stop").or_insert_with(|| json!([]));

    if let Some(legacy) = legacy {
        if let Some(legacy_arr) = legacy.as_array() {
            let arr = stop.as_array_mut().unwrap();
            for entry in legacy_arr {
                if !is_z_hook(entry) {
                    if entry.get("hooks").is_some() {
                        arr.push(entry.clone());
                    } else if let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) {
                        arr.push(json!({
                            "matcher": "",
                            "hooks": [{ "type": "command", "command": cmd }]
                        }));
                    }
                }
            }
        }
    }

    let arr = stop.as_array_mut().unwrap();

    // Remove any existing Z hook, then append the new one.
    arr.retain(|entry| !is_z_hook(entry));
    arr.push(hook_entry);

    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CMD: &str = "zigzag notify \"Claude a terminé: ${ZIGZAG_SESSION_NAME:-$ZELLIJ_SESSION_NAME}\"";

    fn z_hook_entry(cmd: &str) -> Value {
        json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": cmd }]
        })
    }

    #[test]
    fn merge_into_none_creates_hook() {
        let result = merge_stop_hook(None, CMD);
        assert_eq!(
            result,
            json!({
                "hooks": {
                    "Stop": [z_hook_entry(CMD)]
                }
            })
        );
    }

    #[test]
    fn merge_into_existing_no_hooks() {
        let existing = json!({ "permissions": { "allow": ["Read"] } });
        let result = merge_stop_hook(Some(existing), CMD);
        assert_eq!(
            result,
            json!({
                "permissions": { "allow": ["Read"] },
                "hooks": {
                    "Stop": [z_hook_entry(CMD)]
                }
            })
        );
    }

    #[test]
    fn merge_preserves_other_hooks() {
        let existing = json!({
            "hooks": {
                "PreToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": "echo pre" }] }]
            }
        });
        let result = merge_stop_hook(Some(existing), CMD);
        assert_eq!(
            result["hooks"]["PreToolUse"],
            json!([{ "matcher": "", "hooks": [{ "type": "command", "command": "echo pre" }] }])
        );
        assert_eq!(result["hooks"]["Stop"], json!([z_hook_entry(CMD)]));
    }

    #[test]
    fn merge_preserves_non_z_stop_hooks() {
        let existing = json!({
            "hooks": {
                "Stop": [
                    z_hook_entry("echo done"),
                    z_hook_entry("notify-send finished")
                ]
            }
        });
        let result = merge_stop_hook(Some(existing), CMD);
        let stop = result["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 3);
        assert_eq!(stop[0], z_hook_entry("echo done"));
        assert_eq!(stop[1], z_hook_entry("notify-send finished"));
        assert_eq!(stop[2], z_hook_entry(CMD));
    }

    #[test]
    fn merge_updates_existing_z_hook() {
        let existing = json!({
            "hooks": {
                "Stop": [
                    z_hook_entry("echo done"),
                    z_hook_entry("zigzag notify \"old message\"")
                ]
            }
        });
        let result = merge_stop_hook(Some(existing), CMD);
        let stop = result["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], z_hook_entry("echo done"));
        assert_eq!(stop[1], z_hook_entry(CMD));
    }

    #[test]
    fn merge_preserves_unrelated_settings() {
        let existing = json!({
            "permissions": { "allow": ["Read", "Edit"] },
            "model": "opus",
            "hooks": {
                "PreToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": "lint" }] }],
                "Stop": [z_hook_entry("echo bye")]
            }
        });
        let result = merge_stop_hook(Some(existing), CMD);
        assert_eq!(result["permissions"], json!({ "allow": ["Read", "Edit"] }));
        assert_eq!(result["model"], json!("opus"));
        assert_eq!(
            result["hooks"]["PreToolUse"],
            json!([{ "matcher": "", "hooks": [{ "type": "command", "command": "lint" }] }])
        );
        let stop = result["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], z_hook_entry("echo bye"));
        assert_eq!(stop[1], z_hook_entry(CMD));
    }
}
