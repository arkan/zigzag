use crate::domain::{
    AgentActivityState, AgentActivityStatus, NotificationRecord, NotificationSource, NotifyLevel,
    WorktreeIdentity, WorktreeMetadataFile,
};

/// Current metadata schema version after adding agent activity status.
pub const WORKTREE_METADATA_VERSION: u32 = 2;

/// Runtime knobs for agent activity transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentActivitySettings {
    pub working_update_min_interval_ms: u64,
}

impl AgentActivitySettings {
    pub fn from_seconds(working_update_min_interval_seconds: u64) -> Self {
        Self {
            working_update_min_interval_ms: working_update_min_interval_seconds
                .saturating_mul(1000),
        }
    }
}

/// Structured event accepted by `z notify --event`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentActivityEvent {
    Working,
    Waiting { level: NotifyLevel, message: String },
    Idle,
}

/// Input for a single metadata transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivityUpdate {
    pub target: WorktreeIdentity,
    pub tool: String,
    pub event: AgentActivityEvent,
    pub reason: Option<String>,
    pub now_ms: u64,
    pub notification_id: String,
}

/// Result of applying an activity update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentActivityMutation {
    pub changed: bool,
}

/// Apply an agent activity update to an in-memory metadata file.
pub fn apply_agent_activity_update(
    file: &mut WorktreeMetadataFile,
    update: AgentActivityUpdate,
    settings: AgentActivitySettings,
) -> AgentActivityMutation {
    let mut changed = ensure_metadata_version(file);
    let auto_resolve_key = auto_resolve_key(&update.tool);

    match update.event {
        AgentActivityEvent::Working => {
            changed |= clear_auto_resolvable_notifications(
                file,
                &update.target,
                &update.tool,
                &auto_resolve_key,
            );
            changed |= upsert_status(
                file,
                UpsertStatusArgs {
                    target: update.target,
                    tool: update.tool,
                    state: AgentActivityState::Working,
                    now_ms: update.now_ms,
                    reason: update.reason,
                    auto_resolve_key: Some(auto_resolve_key),
                    coalesce_interval_ms: Some(settings.working_update_min_interval_ms),
                },
            );
        }
        AgentActivityEvent::Waiting { level, message } => {
            changed |= upsert_status(
                file,
                UpsertStatusArgs {
                    target: update.target.clone(),
                    tool: update.tool.clone(),
                    state: AgentActivityState::Waiting,
                    now_ms: update.now_ms,
                    reason: update.reason.clone(),
                    auto_resolve_key: Some(auto_resolve_key.clone()),
                    coalesce_interval_ms: None,
                },
            );
            changed |= upsert_waiting_notification(
                file,
                UpsertWaitingNotificationArgs {
                    target: update.target,
                    tool: update.tool,
                    reason: update.reason,
                    auto_resolve_key,
                    level,
                    message,
                    now_ms: update.now_ms,
                    notification_id: update.notification_id,
                },
            );
        }
        AgentActivityEvent::Idle => {
            changed |= clear_auto_resolvable_notifications(
                file,
                &update.target,
                &update.tool,
                &auto_resolve_key,
            );
            let before = file.llm_status.len();
            file.llm_status
                .retain(|status| !(status.target == update.target && status.tool == update.tool));
            changed |= before != file.llm_status.len();
        }
    }

    AgentActivityMutation { changed }
}

/// Return true when a stored working status is still fresh enough to display.
pub fn working_status_is_fresh(status: &AgentActivityStatus, now_ms: u64, ttl_ms: u64) -> bool {
    matches!(status.state, AgentActivityState::Working)
        && now_ms.saturating_sub(status.updated_at_ms) <= ttl_ms
}

pub fn auto_resolve_key(tool: &str) -> String {
    format!("{tool}:waiting")
}

fn ensure_metadata_version(file: &mut WorktreeMetadataFile) -> bool {
    if file.version < WORKTREE_METADATA_VERSION {
        file.version = WORKTREE_METADATA_VERSION;
        return true;
    }
    false
}

struct UpsertStatusArgs {
    target: WorktreeIdentity,
    tool: String,
    state: AgentActivityState,
    now_ms: u64,
    reason: Option<String>,
    auto_resolve_key: Option<String>,
    coalesce_interval_ms: Option<u64>,
}

fn upsert_status(file: &mut WorktreeMetadataFile, args: UpsertStatusArgs) -> bool {
    let UpsertStatusArgs {
        target,
        tool,
        state,
        now_ms,
        reason,
        auto_resolve_key,
        coalesce_interval_ms,
    } = args;
    if let Some(status) = file
        .llm_status
        .iter_mut()
        .find(|status| status.target == target && status.tool == tool)
    {
        if coalesce_interval_ms.is_some_and(|interval| {
            status.state == state && now_ms.saturating_sub(status.updated_at_ms) < interval
        }) {
            return false;
        }

        let next = AgentActivityStatus {
            target,
            tool,
            state,
            updated_at_ms: now_ms,
            reason,
            auto_resolve_key,
        };
        if *status == next {
            return false;
        }
        *status = next;
        true
    } else {
        file.llm_status.push(AgentActivityStatus {
            target,
            tool,
            state,
            updated_at_ms: now_ms,
            reason,
            auto_resolve_key,
        });
        true
    }
}

fn clear_auto_resolvable_notifications(
    file: &mut WorktreeMetadataFile,
    target: &WorktreeIdentity,
    tool: &str,
    auto_resolve_key: &str,
) -> bool {
    let before = file.notifications.len();
    file.notifications.retain(|notification| {
        !notification_source_matches(notification, target, tool, auto_resolve_key, true)
    });
    before != file.notifications.len()
}

struct UpsertWaitingNotificationArgs {
    target: WorktreeIdentity,
    tool: String,
    reason: Option<String>,
    auto_resolve_key: String,
    level: NotifyLevel,
    message: String,
    now_ms: u64,
    notification_id: String,
}

fn upsert_waiting_notification(
    file: &mut WorktreeMetadataFile,
    args: UpsertWaitingNotificationArgs,
) -> bool {
    let UpsertWaitingNotificationArgs {
        target,
        tool,
        reason,
        auto_resolve_key,
        level,
        message,
        now_ms,
        notification_id,
    } = args;
    let auto_resolve = level != NotifyLevel::Error;
    if let Some(notification) = file.notifications.iter_mut().find(|notification| {
        notification_source_matches(
            notification,
            &target,
            &tool,
            &auto_resolve_key,
            auto_resolve,
        )
    }) {
        let next = NotificationRecord {
            id: notification.id.clone(),
            target,
            level,
            message,
            created_at: now_ms,
            source: Some(NotificationSource {
                tool,
                event: "llm.waiting".to_string(),
                reason,
                auto_resolve_key: Some(auto_resolve_key),
                auto_resolve,
            }),
        };
        if *notification == next {
            return false;
        }
        *notification = next;
        true
    } else {
        file.notifications.push(NotificationRecord {
            id: notification_id,
            target,
            level,
            message,
            created_at: now_ms,
            source: Some(NotificationSource {
                tool,
                event: "llm.waiting".to_string(),
                reason,
                auto_resolve_key: Some(auto_resolve_key),
                auto_resolve,
            }),
        });
        true
    }
}

fn notification_source_matches(
    notification: &NotificationRecord,
    target: &WorktreeIdentity,
    tool: &str,
    auto_resolve_key: &str,
    auto_resolve: bool,
) -> bool {
    notification.target == *target
        && notification.source.as_ref().is_some_and(|source| {
            source.tool == tool
                && source.event == "llm.waiting"
                && source.auto_resolve == auto_resolve
                && source.auto_resolve_key.as_deref() == Some(auto_resolve_key)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn identity() -> WorktreeIdentity {
        WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo/app"),
            worktree_path: PathBuf::from("/repo/app"),
        }
    }

    fn empty_file() -> WorktreeMetadataFile {
        WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        }
    }

    fn settings() -> AgentActivitySettings {
        AgentActivitySettings::from_seconds(5)
    }

    #[test]
    fn working_creates_status_without_notification() {
        let mut file = empty_file();

        let mutation = apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Working,
                reason: Some("session.status".to_string()),
                now_ms: 10_000,
                notification_id: "n1".to_string(),
            },
            settings(),
        );

        assert!(mutation.changed);
        assert_eq!(file.version, WORKTREE_METADATA_VERSION);
        assert_eq!(file.llm_status.len(), 1);
        assert_eq!(file.llm_status[0].state, AgentActivityState::Working);
        assert!(file.notifications.is_empty());
    }

    #[test]
    fn repeated_working_inside_interval_is_coalesced() {
        let mut file = empty_file();
        let first = AgentActivityUpdate {
            target: identity(),
            tool: "opencode".to_string(),
            event: AgentActivityEvent::Working,
            reason: Some("session.status".to_string()),
            now_ms: 10_000,
            notification_id: "n1".to_string(),
        };
        apply_agent_activity_update(&mut file, first, settings());

        let mutation = apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                now_ms: 11_000,
                notification_id: "n2".to_string(),
                ..AgentActivityUpdate {
                    target: identity(),
                    tool: "opencode".to_string(),
                    event: AgentActivityEvent::Working,
                    reason: Some("session.status".to_string()),
                    now_ms: 0,
                    notification_id: String::new(),
                }
            },
            settings(),
        );

        assert!(!mutation.changed);
        assert_eq!(file.llm_status[0].updated_at_ms, 10_000);
    }

    #[test]
    fn waiting_creates_auto_resolvable_notification() {
        let mut file = empty_file();

        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Waiting {
                    level: NotifyLevel::Warning,
                    message: "OpenCode needs permission".to_string(),
                },
                reason: Some("permission".to_string()),
                now_ms: 20_000,
                notification_id: "n1".to_string(),
            },
            settings(),
        );

        assert_eq!(file.llm_status[0].state, AgentActivityState::Waiting);
        assert_eq!(file.notifications.len(), 1);
        assert!(
            file.notifications[0]
                .source
                .as_ref()
                .expect("source")
                .auto_resolve
        );
    }

    #[test]
    fn working_after_waiting_clears_waiting_notification_even_inside_interval() {
        let mut file = empty_file();
        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Waiting {
                    level: NotifyLevel::Warning,
                    message: "OpenCode needs permission".to_string(),
                },
                reason: Some("permission".to_string()),
                now_ms: 20_000,
                notification_id: "n1".to_string(),
            },
            settings(),
        );

        let mutation = apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Working,
                reason: Some("session.status".to_string()),
                now_ms: 21_000,
                notification_id: "n2".to_string(),
            },
            settings(),
        );

        assert!(mutation.changed);
        assert!(file.notifications.is_empty());
        assert_eq!(file.llm_status[0].state, AgentActivityState::Working);
    }

    #[test]
    fn idle_removes_status_and_auto_resolvable_notification() {
        let mut file = empty_file();
        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Waiting {
                    level: NotifyLevel::Warning,
                    message: "OpenCode needs permission".to_string(),
                },
                reason: Some("permission".to_string()),
                now_ms: 20_000,
                notification_id: "n1".to_string(),
            },
            settings(),
        );

        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: identity(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Idle,
                reason: None,
                now_ms: 30_000,
                notification_id: "n2".to_string(),
            },
            settings(),
        );

        assert!(file.notifications.is_empty());
        assert!(file.llm_status.is_empty());
    }

    #[test]
    fn working_does_not_remove_manual_or_error_notifications() {
        let mut file = empty_file();
        let target = identity();
        file.notifications.push(NotificationRecord {
            id: "manual".to_string(),
            target: target.clone(),
            level: NotifyLevel::Warning,
            message: "manual".to_string(),
            created_at: 1,
            source: None,
        });
        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target: target.clone(),
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Waiting {
                    level: NotifyLevel::Error,
                    message: "OpenCode error".to_string(),
                },
                reason: Some("error".to_string()),
                now_ms: 20_000,
                notification_id: "error".to_string(),
            },
            settings(),
        );

        apply_agent_activity_update(
            &mut file,
            AgentActivityUpdate {
                target,
                tool: "opencode".to_string(),
                event: AgentActivityEvent::Working,
                reason: None,
                now_ms: 30_000,
                notification_id: "n2".to_string(),
            },
            settings(),
        );

        assert_eq!(file.notifications.len(), 2);
        assert!(file
            .notifications
            .iter()
            .any(|notification| notification.id == "manual"));
        assert!(file
            .notifications
            .iter()
            .any(|notification| notification.id == "error"));
    }
}
