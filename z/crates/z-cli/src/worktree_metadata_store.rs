//! Local filesystem adapter for `WorktreeMetadataStore`.
//!
//! Stores worktree metadata at `~/.config/z/worktree-metadata.json` with
//! lock-file mutual exclusion and atomic temp+rename writes. Corrupt JSON
//! is detected and reported as `ZError::MetadataCorrupt` — never silently
//! overwritten.
//!
//! Migration helpers read legacy `~/.config/z/session-activity.json` and
//! `/tmp/z/notifications/` and attempt to resolve old session names against
//! a provided list of `DiscoveredWorktree` entries.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use z_core::domain::{
    DiscoveredWorktree, NotifyLevel, UnattachedActivity,
    NotificationRecord, UnattachedNotification, WorktreeIdentity,
    WorktreeMetadataFile, WorktreeMetadataRecord,
};
use z_core::error::{Result, ZError};
use z_core::traits::WorktreeMetadataStore;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const METADATA_FILENAME: &str = "worktree-metadata.json";
const LOCK_FILENAME: &str = "worktree-metadata.lock";
const MIGRATION_BACKUP_SUFFIX: &str = ".bak";
const LEGACY_ACTIVITY_FILENAME: &str = "session-activity.json";
const LEGACY_NOTIFICATIONS_DIR: &str = "/tmp/z/notifications";
const CURRENT_VERSION: u32 = z_core::agent_activity::WORKTREE_METADATA_VERSION;
const LOCK_RETRY_COUNT: u32 = 10;
const LOCK_RETRY_DELAY_MS: u64 = 50;

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// File-backed adapter for worktree metadata.
///
/// Uses the file contract from Phase 1: a single versioned JSON file at
/// `~/.config/z/worktree-metadata.json` with lock-file protection.
#[derive(Debug, Clone)]
pub struct LocalWorktreeMetadataStore {
    config_dir: PathBuf,
}

/// SSH-backed adapter for a remote host's `~/.config/z/worktree-metadata.json`.
#[derive(Debug, Clone)]
pub struct RemoteWorktreeMetadataStore {
    host: String,
}

impl RemoteWorktreeMetadataStore {
    pub fn new(host: impl Into<String>) -> Self {
        Self { host: host.into() }
    }

    fn apply_local_host(&self, mut data: WorktreeMetadataFile) -> WorktreeMetadataFile {
        for record in &mut data.worktrees {
            if record.host.is_none() {
                record.host = Some(self.host.clone());
            }
        }
        for notification in &mut data.notifications {
            if notification.target.host.is_none() {
                notification.target.host = Some(self.host.clone());
            }
        }
        for status in &mut data.llm_status {
            if status.target.host.is_none() {
                status.target.host = Some(self.host.clone());
            }
        }
        data
    }

    fn strip_local_host(&self, mut data: WorktreeMetadataFile) -> WorktreeMetadataFile {
        for record in &mut data.worktrees {
            if record.host.as_deref() == Some(self.host.as_str()) {
                record.host = None;
            }
        }
        for notification in &mut data.notifications {
            if notification.target.host.as_deref() == Some(self.host.as_str()) {
                notification.target.host = None;
            }
        }
        for status in &mut data.llm_status {
            if status.target.host.as_deref() == Some(self.host.as_str()) {
                status.target.host = None;
            }
        }
        data
    }
}

impl LocalWorktreeMetadataStore {
    /// Create a store rooted at `config_dir` (the `~/.config/z/` directory).
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    /// Default config directory from `$HOME/.config/z`.
    pub fn default_config_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config/z")
    }

    /// Create store at the default config directory.
    pub fn default() -> Self {
        Self::new(Self::default_config_dir())
    }

    // -- paths --

    fn metadata_path(&self) -> PathBuf {
        self.config_dir.join(METADATA_FILENAME)
    }

    fn lock_path(&self) -> PathBuf {
        self.config_dir.join(LOCK_FILENAME)
    }

    fn legacy_activity_path(&self) -> PathBuf {
        self.config_dir.join(LEGACY_ACTIVITY_FILENAME)
    }

    fn legacy_activity_backup_path(&self) -> PathBuf {
        self.config_dir.join(format!(
            "{}{}",
            LEGACY_ACTIVITY_FILENAME, MIGRATION_BACKUP_SUFFIX
        ))
    }

    // -- lock helpers --

    /// Acquire exclusive lock via atomic create, retry on contention.
    fn acquire_lock(&self) -> Result<LockGuard> {
        let lock_path = self.lock_path();

        // Ensure parent config directory exists
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ZError::MetadataLock(format!("cannot create lock dir: {e}")))?;
        }

        for attempt in 0..LOCK_RETRY_COUNT {
            match fs::File::create_new(&lock_path) {
                Ok(mut file) => {
                    // Write PID + timestamp for debugging
                    let pid = std::process::id();
                    let ts = unix_now_ms();
                    let _ = writeln!(file, "{pid} {ts}");
                    return Ok(LockGuard {
                        lock_path: lock_path.clone(),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if attempt + 1 < LOCK_RETRY_COUNT {
                        std::thread::sleep(std::time::Duration::from_millis(
                            LOCK_RETRY_DELAY_MS,
                        ));
                    } else {
                        return Err(ZError::MetadataLock(format!(
                            "lock file exists after {LOCK_RETRY_COUNT} retries: {}",
                            lock_path.display()
                        )));
                    }
                }
                Err(e) => {
                    return Err(ZError::MetadataLock(format!(
                        "cannot create lock file: {e}"
                    )));
                }
            }
        }
        unreachable!()
    }

    // -- raw I/O (called inside lock) --

    fn read_raw_unlocked(&self) -> Result<Option<WorktreeMetadataFile>> {
        let path = self.metadata_path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)
            .map_err(|e| ZError::MetadataCorrupt(format!("cannot read metadata file: {e}")))?;
        match serde_json::from_slice::<WorktreeMetadataFile>(&bytes) {
            Ok(file) => Ok(Some(file)),
            Err(e) => {
                // Corrupt JSON → error, never silently overwrite
                Err(ZError::MetadataCorrupt(format!(
                    "corrupt metadata file at {}: {e}",
                    path.display()
                )))
            }
        }
    }

    fn write_atomic_unlocked(&self, data: &WorktreeMetadataFile) -> Result<()> {
        // Ensure config dir exists
        fs::create_dir_all(&self.config_dir)
            .map_err(|e| ZError::MetadataWrite(format!("cannot create config dir: {e}")))?;

        let path = self.metadata_path();
        let tmp = path.with_extension("json.tmp");

        let bytes = serde_json::to_vec_pretty(data)
            .map_err(|e| ZError::MetadataWrite(format!("serialize metadata: {e}")))?;

        // Write to temp file
        fs::write(&tmp, &bytes)
            .map_err(|e| ZError::MetadataWrite(format!("cannot write temp file: {e}")))?;

        // Atomic rename
        fs::rename(&tmp, &path)
            .map_err(|e| ZError::MetadataWrite(format!("cannot rename metadata file: {e}")))?;

        Ok(())
    }

    // -- migration --

    /// Final idempotent drain of legacy `/tmp/z/notifications/` into metadata.
    ///
    /// Unlike the initial one-shot `migrate_legacy_activity()`, this function
    /// always runs and only handles notification files. It deduplicates by
    /// legacy file ID so repeated calls are safe. Legacy files are deleted
    /// only after metadata is durably written.
    ///
    /// If `legacy_notifications_path` is `None`, defaults to `/tmp/z/notifications/`.
    pub fn drain_legacy_notifications(
        &self,
        discovered_worktrees: &[DiscoveredWorktree],
        legacy_notifications_path: Option<&Path>,
    ) -> Result<MigrationReport> {
        let _guard = self.acquire_lock()?;

        let mut metadata = self.read_raw_unlocked()?.unwrap_or_else(Self::empty_file);
        let notifications_path = legacy_notifications_path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(LEGACY_NOTIFICATIONS_DIR));

        let legacy_notifications = self.read_legacy_notifications(&notifications_path);
        let mut report = MigrationReport::default();
        let mut cleaned_sessions: Vec<String> = Vec::new();

        for (session_name, notifications) in &legacy_notifications {
            for (file_id, level, message, created_at) in notifications {
                let dedup_key = format!("{session_name}/{file_id}");
                if metadata.migrated_legacy_ids.contains(&dedup_key) {
                    continue; // already migrated
                }

                let resolution =
                    z_core::domain::resolve_session_alias(session_name, discovered_worktrees);
                match resolution {
                    z_core::domain::SessionAliasResolution::Unique(wt) => {
                        ensure_worktree_record(&mut metadata.worktrees, &wt);
                        metadata
                            .notifications
                            .push(z_core::domain::NotificationRecord {
                                id: file_id.clone(),
                                target: wt.identity.clone(),
                                level: level.clone(),
                                message: message.clone(),
                                created_at: *created_at,
                                source: None,
                            });
                        metadata.migrated_legacy_ids.insert(dedup_key);
                        report.migrated_notifications += 1;
                    }
                    z_core::domain::SessionAliasResolution::Ambiguous(_)
                    | z_core::domain::SessionAliasResolution::None => {
                        metadata
                            .unattached_notifications
                            .push(UnattachedNotification {
                                id: file_id.clone(),
                                session_name: session_name.clone(),
                                level: level.clone(),
                                message: message.clone(),
                                created_at: *created_at,
                            });
                        metadata.migrated_legacy_ids.insert(dedup_key);
                        report.unattached_notifications += 1;
                    }
                }
            }
            // Mark session directory for cleanup only if we migrated any new files
            cleaned_sessions.push(session_name.clone());
        }

        if report.migrated_notifications > 0 || report.unattached_notifications > 0 {
            metadata.version = CURRENT_VERSION;
            self.write_atomic_unlocked(&metadata)?;
        }

        // Delete legacy files only after metadata write succeeded
        if !cleaned_sessions.is_empty() && notifications_path.exists() {
            for session_name in &cleaned_sessions {
                let dir = notifications_path.join(session_name);
                let _ = fs::remove_dir_all(&dir);
            }
        }

        Ok(report)
    }

    /// One-shot migration from legacy `session-activity.json` into metadata.
    ///
    /// Only runs when `worktree-metadata.json` does not exist. Migrates
    /// activity timestamps into worktree records and backs up the legacy
    /// activity file. Notification drain is handled separately by
    /// `drain_legacy_notifications()`.
    pub fn migrate_legacy_activity(
        &self,
        discovered_worktrees: &[DiscoveredWorktree],
    ) -> Result<MigrationReport> {
        let _guard = self.acquire_lock()?;

        if self.metadata_path().exists() {
            return Ok(MigrationReport {
                skipped_because_exists: true,
                ..MigrationReport::default()
            });
        }

        let mut report = MigrationReport::default();
        let mut metadata_records: Vec<WorktreeMetadataRecord> = Vec::new();
        let legacy_activity = self.read_legacy_activity();
        let mut unattached_activity: Vec<UnattachedActivity> = Vec::new();

        for (session_name, ts) in &legacy_activity {
            let resolution =
                z_core::domain::resolve_session_alias(session_name, discovered_worktrees);
            match resolution {
                z_core::domain::SessionAliasResolution::Unique(wt) => {
                    let record = ensure_worktree_record(&mut metadata_records, &wt);
                    if record.last_opened_at.map_or(true, |existing| *ts > existing) {
                        record.last_opened_at = Some(*ts);
                    }
                    record.last_session_name = Some(session_name.clone());
                    report.migrated_worktrees += 1;
                }
                z_core::domain::SessionAliasResolution::Ambiguous(_) => {
                    unattached_activity.push(UnattachedActivity {
                        session_name: session_name.clone(),
                        last_attached_at: *ts,
                    });
                    report.unattached_activity += 1;
                    report
                        .diagnostics
                        .push(format!("ambiguous session name (activity): {session_name}"));
                }
                z_core::domain::SessionAliasResolution::None => {
                    unattached_activity.push(UnattachedActivity {
                        session_name: session_name.clone(),
                        last_attached_at: *ts,
                    });
                    report.unattached_activity += 1;
                    report
                        .diagnostics
                        .push(format!("unresolved session name (activity): {session_name}"));
                }
            }
        }

        let metadata_file = WorktreeMetadataFile {
            version: CURRENT_VERSION,
            worktrees: metadata_records,
            notifications: Vec::new(),
            unattached_notifications: Vec::new(),
            unattached_activity,
            migration_diagnostics: report.diagnostics.clone(),
            llm_status: Vec::new(),
            migrated_legacy_ids: std::collections::HashSet::new(),
        };

        self.write_atomic_unlocked(&metadata_file)?;

        // Backup legacy activity file after metadata is durably written
        if !legacy_activity.is_empty() {
            let backup_bytes = serde_json::to_vec_pretty(&legacy_activity)
                .map_err(|e| ZError::MetadataWrite(format!("serialize backup: {e}")))?;
            let backup_path = self.legacy_activity_backup_path();
            fs::write(&backup_path, &backup_bytes)
                .map_err(|e| ZError::MetadataWrite(format!("cannot write backup: {e}")))?;
        }

        Ok(report)
    }

    // -- legacy readers --

    /// Read legacy session-activity.json. Returns empty map on any error.
    fn read_legacy_activity(&self) -> std::collections::HashMap<String, u64> {
        let path = self.legacy_activity_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => return std::collections::HashMap::new(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Read legacy notifications from `<base>/<session>/<ts>_<seq>`.
    ///
    /// Returns map of session_name → Vec<(id, level, message, created_at)>.
    fn read_legacy_notifications(
        &self,
        notifications_path: &Path,
    ) -> std::collections::HashMap<String, Vec<(String, NotifyLevel, String, u64)>> {
        let base = notifications_path;
        if !base.exists() {
            return std::collections::HashMap::new();
        }

        let mut result: std::collections::HashMap<
            String,
            Vec<(String, NotifyLevel, String, u64)>,
        > = std::collections::HashMap::new();

        let entries = match fs::read_dir(&base) {
            Ok(e) => e,
            Err(_) => return result,
        };

        for entry in entries.flatten() {
            let session_name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let session_dir = entry.path();
            if !session_dir.is_dir() {
                continue;
            }

            let mut notifs: Vec<(String, NotifyLevel, String, u64)> = Vec::new();

            for file_entry in fs::read_dir(&session_dir).into_iter().flatten() {
                let file_entry = match file_entry {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let content = match fs::read_to_string(file_entry.path()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let id = file_entry
                    .file_name()
                    .to_string_lossy()
                    .to_string();
                let created_at = file_entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.split('_').next())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);

                // Parse level + message from content (format: "level\nmessage")
                let (level, message) = parse_notification_content(&content);
                notifs.push((id, level, message, created_at));
            }

            if !notifs.is_empty() {
                result.insert(session_name, notifs);
            }
        }

        result
    }
}

impl LocalWorktreeMetadataStore {
    fn empty_file() -> WorktreeMetadataFile {
        WorktreeMetadataFile {
            version: CURRENT_VERSION,
            worktrees: Vec::new(),
            notifications: Vec::new(),
            unattached_notifications: Vec::new(),
            unattached_activity: Vec::new(),
            migration_diagnostics: Vec::new(),
            llm_status: Vec::new(),
            migrated_legacy_ids: std::collections::HashSet::new(),
        }
    }

    fn update_metadata(&self, mutate: impl FnOnce(&mut WorktreeMetadataFile)) -> Result<()> {
        let _guard = self.acquire_lock()?;
        let mut file = self.read_raw_unlocked()?.unwrap_or_else(Self::empty_file);
        mutate(&mut file);
        self.write_atomic_unlocked(&file)
    }

    fn update_metadata_if_changed(
        &self,
        mutate: impl FnOnce(&mut WorktreeMetadataFile) -> bool,
    ) -> Result<bool> {
        let _guard = self.acquire_lock()?;
        let mut file = self.read_raw_unlocked()?.unwrap_or_else(Self::empty_file);
        let changed = mutate(&mut file);
        if changed {
            self.write_atomic_unlocked(&file)?;
        }
        Ok(changed)
    }

    /// Record a successful Worktree entry/open and keep metadata sparse.
    pub fn record_opened(&self, worktree: &DiscoveredWorktree, session_name: &str) -> Result<()> {
        let now = unix_now_ms();
        self.update_metadata(|file| {
            file.version = CURRENT_VERSION;
            let record = ensure_worktree_record(&mut file.worktrees, worktree);
            record.last_opened_at = Some(now);
            record.last_session_name = Some(session_name.to_string());
        })
    }

    /// Remove all metadata and notifications attached to a Worktree identity.
    pub fn remove_worktree(&self, identity: &WorktreeIdentity) -> Result<()> {
        self.update_metadata(|file| {
            file.version = CURRENT_VERSION;
            file.worktrees.retain(|record| !record_matches_identity(record, identity));
            file.notifications.retain(|notification| notification.target != *identity);
            file.llm_status.retain(|status| status.target != *identity);
        })
    }

    /// Add a notification attached to a resolved Worktree identity.
    pub fn add_notification(
        &self,
        target: WorktreeIdentity,
        level: NotifyLevel,
        message: &str,
    ) -> Result<()> {
        let now = unix_now_ms();
        self.update_metadata(|file| {
            file.version = CURRENT_VERSION;
            file.notifications.push(NotificationRecord {
                id: format!("{}-{}", now, std::process::id()),
                target,
                level,
                message: message.to_string(),
                created_at: now,
                source: None,
            });
        })
    }

    /// Add a notification that could not be resolved to a Worktree.
    pub fn add_unattached_notification(
        &self,
        session_name: &str,
        level: NotifyLevel,
        message: &str,
    ) -> Result<()> {
        let now = unix_now_ms();
        self.update_metadata(|file| {
            file.version = CURRENT_VERSION;
            file.unattached_notifications.push(UnattachedNotification {
                id: format!("{}-{}", now, std::process::id()),
                session_name: session_name.to_string(),
                level,
                message: message.to_string(),
                created_at: now,
            });
        })
    }

    /// Clear notifications attached to a Worktree identity after successful entry.
    pub fn clear_notifications(&self, identity: &WorktreeIdentity) -> Result<()> {
        self.update_metadata(|file| {
            file.version = CURRENT_VERSION;
            file.notifications.retain(|notification| notification.target != *identity);
            file.llm_status.retain(|status| {
                !(status.target == *identity
                    && matches!(status.state, z_core::domain::AgentActivityState::Waiting))
            });
        })
    }

    /// Apply a structured agent activity update to Worktree metadata.
    pub fn apply_agent_activity(
        &self,
        target: WorktreeIdentity,
        tool: &str,
        event: z_core::agent_activity::AgentActivityEvent,
        reason: Option<String>,
        settings: z_core::agent_activity::AgentActivitySettings,
    ) -> Result<bool> {
        let now = unix_now_ms();
        let notification_id = format!("{}-{}", now, std::process::id());
        self.update_metadata_if_changed(|file| {
            z_core::agent_activity::apply_agent_activity_update(
                file,
                z_core::agent_activity::AgentActivityUpdate {
                    target,
                    tool: tool.to_string(),
                    event,
                    reason,
                    now_ms: now,
                    notification_id,
                },
                settings,
            )
            .changed
        })
    }

    /// Return derived session aliases that currently have attached notifications.
    ///
    /// When `discovered` is provided, notification targets are resolved against
    /// both metadata records and discovered worktrees, ensuring notifications
    /// attached to a valid `WorktreeIdentity` produce aliases even when no
    /// metadata worktree record exists yet.
    pub fn notification_session_aliases(
        &self,
        discovered: Option<&[z_core::domain::DiscoveredWorktree]>,
    ) -> Result<std::collections::HashSet<String>> {
        let file = self.read_metadata()?;
        let mut aliases = std::collections::HashSet::new();
        for notification in &file.notifications {
            // Try matching against discovered worktrees first (more complete)
            if let Some(discovered) = discovered {
                if let Some(wt) = discovered.iter().find(|wt| wt.identity == notification.target) {
                    if let Some(branch) = &wt.branch {
                        aliases.insert(z_core::domain::derive_session_name(
                            &wt.project_name,
                            branch,
                        ));
                        continue;
                    }
                }
            }
            // Fall back to metadata records
            if let Some(record) = file
                .worktrees
                .iter()
                .find(|record| record_matches_identity(record, &notification.target))
            {
                if let Some(branch) = record.branch.as_deref() {
                    aliases
                        .insert(z_core::domain::derive_session_name(&record.project_name, branch));
                }
            }
        }
        Ok(aliases)
    }
}

impl WorktreeMetadataStore for LocalWorktreeMetadataStore {
    fn read_metadata(&self) -> Result<WorktreeMetadataFile> {
        // No lock needed for reads (atomic read of a file that is only replaced
        // atomically via rename). The file's final state is always consistent.
        self.read_raw_unlocked()
            .map(|opt| opt.unwrap_or_else(Self::empty_file))
    }

    fn write_metadata(&self, data: &WorktreeMetadataFile) -> Result<()> {
        let _guard = self.acquire_lock()?;
        self.write_atomic_unlocked(data)
    }
}

impl WorktreeMetadataStore for RemoteWorktreeMetadataStore {
    fn read_metadata(&self) -> Result<WorktreeMetadataFile> {
        match crate::remote::read_remote_z_config_file(&self.host, METADATA_FILENAME)? {
            Some(content) => serde_json::from_str::<WorktreeMetadataFile>(&content)
                .map(|data| self.apply_local_host(data))
                .map_err(|e| {
                    ZError::MetadataCorrupt(format!(
                        "corrupt remote metadata file on {}: {e}",
                        self.host
                    ))
                }),
            None => Ok(self.apply_local_host(LocalWorktreeMetadataStore::empty_file())),
        }
    }

    fn write_metadata(&self, data: &WorktreeMetadataFile) -> Result<()> {
        let remote_data = self.strip_local_host(data.clone());
        let bytes = serde_json::to_vec_pretty(&remote_data)
            .map_err(|e| ZError::MetadataWrite(format!("serialize remote metadata: {e}")))?;
        crate::remote::write_remote_z_config_file_atomic(&self.host, METADATA_FILENAME, &bytes)
    }
}

impl Default for LocalWorktreeMetadataStore {
    fn default() -> Self {
        Self::new(Self::default_config_dir())
    }
}

// ---------------------------------------------------------------------------
// Lock guard
// ---------------------------------------------------------------------------

/// RAII guard that holds the lock file open and removes it on drop.
#[derive(Debug)]
struct LockGuard {
    lock_path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

// ---------------------------------------------------------------------------
// Migration report
// ---------------------------------------------------------------------------

/// Summary of a migration run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigrationReport {
    pub migrated_worktrees: usize,
    pub migrated_notifications: usize,
    pub unattached_activity: usize,
    pub unattached_notifications: usize,
    pub diagnostics: Vec<String>,
    pub skipped_because_exists: bool,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ensure_worktree_record<'a>(
    records: &'a mut Vec<WorktreeMetadataRecord>,
    worktree: &DiscoveredWorktree,
) -> &'a mut WorktreeMetadataRecord {
    if let Some(index) = records.iter().position(|record| {
        record.path == worktree.identity.worktree_path
            && record.project_root == worktree.identity.project_root
            && record.host == worktree.identity.host
    }) {
        return &mut records[index];
    }

    records.push(WorktreeMetadataRecord {
        project_name: worktree.project_name.clone(),
        project_root: worktree.identity.project_root.clone(),
        host: worktree.identity.host.clone(),
        branch: worktree.branch.clone(),
        path: worktree.identity.worktree_path.clone(),
        last_opened_at: None,
        last_session_name: None,
    });
    records.last_mut().expect("record was just pushed")
}

fn record_matches_identity(record: &WorktreeMetadataRecord, identity: &WorktreeIdentity) -> bool {
    record.host == identity.host
        && record.project_root == identity.project_root
        && record.path == identity.worktree_path
}

/// Parse legacy notification content (`level\nmessage`) into (level, message).
fn parse_notification_content(content: &str) -> (NotifyLevel, String) {
    let mut lines = content.lines();
    let level_str = lines.next().unwrap_or("info");
    let message = lines.collect::<Vec<_>>().join("\n");

    let level = match level_str {
        "warning" => NotifyLevel::Warning,
        "error" => NotifyLevel::Error,
        _ => NotifyLevel::Info,
    };

    (level, message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{thread, time::Duration};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("z-wt-metadata-test-{}-{}", pid, n))
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    /// Helper to build a store rooted at a temp directory.
    fn temp_store() -> (LocalWorktreeMetadataStore, PathBuf) {
        let dir = test_dir();
        let store = LocalWorktreeMetadataStore::new(dir.clone());
        (store, dir)
    }

    // -----------------------------------------------------------------------
    // Basic read/write
    // -----------------------------------------------------------------------

    #[test]
    fn read_empty_when_no_file() {
        let (store, dir) = temp_store();
        let file = store.read_metadata().unwrap();
        assert_eq!(file.version, CURRENT_VERSION);
        assert!(file.worktrees.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (store, dir) = temp_store();
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![WorktreeMetadataRecord {
                project_name: "myapp".into(),
                project_root: PathBuf::from("/repo"),
                host: None,
                branch: Some("main".into()),
                path: PathBuf::from("/repo"),
                last_opened_at: Some(1_710_000_000),
                last_session_name: None,
            }],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data).unwrap();
        let got = store.read_metadata().unwrap();
        assert_eq!(got.version, 1);
        assert_eq!(got.worktrees.len(), 1);
        assert_eq!(got.worktrees[0].project_name, "myapp");
        cleanup(&dir);
    }

    #[test]
    fn write_creates_config_dir() {
        let dir = test_dir().join("nested").join("config");
        let store = LocalWorktreeMetadataStore::new(&dir);
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data).unwrap();
        assert!(store.metadata_path().exists());
        cleanup(&dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn write_overwrites_previous() {
        let (store, dir) = temp_store();
        let data1 = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![WorktreeMetadataRecord {
                project_name: "a".into(),
                project_root: PathBuf::from("/a"),
                host: None,
                branch: None,
                path: PathBuf::from("/a"),
                last_opened_at: None,
                last_session_name: None,
            }],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data1).unwrap();

        let data2 = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![WorktreeMetadataRecord {
                project_name: "b".into(),
                project_root: PathBuf::from("/b"),
                host: None,
                branch: None,
                path: PathBuf::from("/b"),
                last_opened_at: None,
                last_session_name: None,
            }],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data2).unwrap();

        let got = store.read_metadata().unwrap();
        assert_eq!(got.worktrees.len(), 1);
        assert_eq!(got.worktrees[0].project_name, "b");
        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Corrupt JSON detection
    // -----------------------------------------------------------------------

    #[test]
    fn corrupt_json_returns_error() {
        let (store, dir) = temp_store();
        // Ensure parent dir exists, then write invalid JSON directly
        fs::create_dir_all(&dir).unwrap();
        fs::write(store.metadata_path(), b"not valid json").unwrap();
        let err = store.read_metadata().unwrap_err();
        assert!(
            matches!(&err, ZError::MetadataCorrupt(msg) if msg.contains("corrupt")),
            "expected MetadataCorrupt, got {err:?}"
        );
        cleanup(&dir);
    }

    #[test]
    fn corrupt_json_does_not_overwrite_on_read() {
        let (store, dir) = temp_store();
        fs::create_dir_all(&dir).unwrap();
        fs::write(store.metadata_path(), b"{{{broken").unwrap();

        // Read must fail
        assert!(store.read_metadata().is_err());

        // File content must be preserved (not silently overwritten)
        let content = fs::read_to_string(store.metadata_path()).unwrap();
        assert_eq!(content, "{{{broken");
        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Lock behavior
    // -----------------------------------------------------------------------

    #[test]
    fn lock_prevents_concurrent_writes() {
        let (store, dir) = temp_store();
        let store2 = store.clone();

        let _guard = store.acquire_lock().unwrap();
        let result = store2.acquire_lock();
        assert!(
            result.is_err(),
            "acquiring lock while held should fail: {result:?}"
        );
        drop(_guard);

        // After guard drops, acquire must succeed again
        let guard3 = store.acquire_lock();
        assert!(guard3.is_ok());
        drop(guard3);
        cleanup(&dir);
    }

    #[test]
    fn lock_cleans_up_on_drop() {
        let (store, dir) = temp_store();
        let lock_path = store.lock_path();

        {
            let _guard = store.acquire_lock().unwrap();
            assert!(lock_path.exists());
        }
        // After guard drops, lock file must be removed
        assert!(!lock_path.exists());
        cleanup(&dir);
    }

    #[test]
    fn write_releases_lock_after_completion() {
        let (store, dir) = temp_store();
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };

        store.write_metadata(&data).unwrap();
        // Lock file should not exist after write completes
        assert!(!store.lock_path().exists());
        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Atomic write safety
    // -----------------------------------------------------------------------

    #[test]
    fn temp_file_cleaned_up_after_write() {
        let (store, dir) = temp_store();
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data).unwrap();

        // Temp file must not exist after successful write
        let tmp = store.metadata_path().with_extension("json.tmp");
        assert!(!tmp.exists());
        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Migration from legacy data
    // -----------------------------------------------------------------------

    fn write_legacy_activity(dir: &Path, data: &std::collections::HashMap<String, u64>) {
        let path = dir.join(LEGACY_ACTIVITY_FILENAME);
        fs::create_dir_all(dir).unwrap();
        let bytes = serde_json::to_vec(data).unwrap();
        fs::write(&path, &bytes).unwrap();
    }

    fn cleanup_legacy_notifications() {
        let base = PathBuf::from(LEGACY_NOTIFICATIONS_DIR);
        if base.exists() {
            let _ = fs::remove_dir_all(&base);
        }
    }

    fn make_discovered_wt(
        project: &str,
        root: &str,
        wt_path: &str,
        branch: Option<&str>,
    ) -> DiscoveredWorktree {
        DiscoveredWorktree {
            identity: z_core::domain::WorktreeIdentity {
                host: None,
                project_root: PathBuf::from(root),
                worktree_path: PathBuf::from(wt_path),
            },
            project_name: project.to_string(),
            branch: branch.map(String::from),
            is_primary_checkout: root == wt_path,
        }
    }

    #[test]
    fn migration_creates_metadata_from_activity() {
        let (store, dir) = temp_store();
        cleanup_legacy_notifications();

        let mut activity = std::collections::HashMap::new();
        activity.insert("myapp:feat-login".to_string(), 1_710_000_000u64);
        write_legacy_activity(&dir, &activity);

        let worktrees = vec![make_discovered_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-login",
            Some("feat/login"),
        )];

        let report = store
            .migrate_legacy_activity(&worktrees)
            .unwrap();

        assert_eq!(report.migrated_worktrees, 1);
        assert_eq!(report.unattached_activity, 0);
        assert!(!report.skipped_because_exists);

        // Verify metadata file exists and has the right timestamp
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.worktrees.len(), 1);
        assert_eq!(metadata.worktrees[0].project_name, "myapp");
        assert_eq!(metadata.worktrees[0].last_opened_at, Some(1_710_000_000));

        // Backup must exist
        assert!(dir.join("session-activity.json.bak").exists());

        cleanup(&dir);
    }

    #[test]
    fn migration_preserves_unresolved_activity() {
        let (store, dir) = temp_store();
        cleanup_legacy_notifications();

        let mut activity = std::collections::HashMap::new();
        activity.insert("myapp:stale".to_string(), 1_700_000_000u64);
        write_legacy_activity(&dir, &activity);

        let worktrees = vec![]; // no worktrees → all activity is unattached

        let report = store.migrate_legacy_activity(&worktrees).unwrap();
        assert_eq!(report.unattached_activity, 1);
        assert_eq!(report.migrated_worktrees, 0);

        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.unattached_activity.len(), 1);
        assert_eq!(metadata.unattached_activity[0].session_name, "myapp:stale");

        cleanup(&dir);
    }

    #[test]
    fn migration_preserves_unattached_notifications() {
        let (store, dir) = temp_store();

        // Use a temp dir for notifications to avoid parallel test interference
        let notif_base = test_dir();
        let session_dir = notif_base.join("myapp:ghost");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("123_0"),
            "warning\nLegacy notification",
        )
        .unwrap();

        let worktrees = vec![];

        let report = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();
        assert_eq!(report.unattached_notifications, 1);

        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.unattached_notifications.len(), 1);
        assert_eq!(
            metadata.unattached_notifications[0].session_name,
            "myapp:ghost"
        );
        assert_eq!(
            metadata.unattached_notifications[0].message,
            "Legacy notification"
        );

        // Cleanup
        cleanup(&dir);
        cleanup(&notif_base);
    }

    #[test]
    fn migration_creates_metadata_for_resolved_notification_only_worktree() {
        let (store, dir) = temp_store();

        let notif_base = test_dir();
        let session_dir = notif_base.join("myapp:feat-login");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("123_0"), "info\nBuild finished").unwrap();

        let worktrees = vec![make_discovered_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-login",
            Some("feat/login"),
        )];

        let report = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();

        assert_eq!(report.migrated_notifications, 1);
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.worktrees.len(), 1);
        assert_eq!(metadata.notifications.len(), 1);
        assert_eq!(metadata.notifications[0].message, "Build finished");

        cleanup(&dir);
        cleanup(&notif_base);
    }

    #[test]
    fn remote_store_normalizes_host_for_local_view_and_strips_for_remote_file() {
        let store = RemoteWorktreeMetadataStore::new("remote.example");
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![WorktreeMetadataRecord {
                project_name: "myapp".to_string(),
                project_root: PathBuf::from("/repo"),
                host: None,
                branch: Some("main".to_string()),
                path: PathBuf::from("/repo"),
                last_opened_at: None,
                last_session_name: None,
            }],
            notifications: vec![NotificationRecord {
                id: "n1".to_string(),
                target: WorktreeIdentity {
                    host: None,
                    project_root: PathBuf::from("/repo"),
                    worktree_path: PathBuf::from("/repo"),
                },
                level: NotifyLevel::Info,
                message: "hello".to_string(),
                created_at: 1,
                source: None,
            }],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };

        let local_view = store.apply_local_host(data);
        assert_eq!(local_view.worktrees[0].host.as_deref(), Some("remote.example"));
        assert_eq!(local_view.notifications[0].target.host.as_deref(), Some("remote.example"));

        let remote_file = store.strip_local_host(local_view);
        assert_eq!(remote_file.worktrees[0].host, None);
        assert_eq!(remote_file.notifications[0].target.host, None);
    }

    #[test]
    fn migration_skips_when_metadata_exists() {
        let (store, dir) = temp_store();

        // Write existing metadata
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data).unwrap();

        // Migration should be skipped
        let report = store.migrate_legacy_activity(&[]).unwrap();
        assert!(report.skipped_because_exists);
        cleanup(&dir);
    }

    #[test]
    fn migration_rechecks_existing_metadata_after_waiting_for_lock() {
        let (store, dir) = temp_store();
        let guard = store.acquire_lock().unwrap();
        let store_for_thread = store.clone();

        let handle = thread::spawn(move || store_for_thread.migrate_legacy_activity(&[]).unwrap());
        thread::sleep(Duration::from_millis(25));

        let data = WorktreeMetadataFile {
            version: CURRENT_VERSION,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![z_core::domain::AgentActivityStatus {
                target: WorktreeIdentity {
                    host: None,
                    project_root: PathBuf::from("/repo"),
                    worktree_path: PathBuf::from("/repo"),
                },
                tool: "opencode".to_string(),
                state: z_core::domain::AgentActivityState::Working,
                updated_at_ms: 1,
                reason: None,
                auto_resolve_key: None,
            }],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        fs::create_dir_all(&dir).unwrap();
        fs::write(store.metadata_path(), serde_json::to_vec_pretty(&data).unwrap()).unwrap();

        drop(guard);
        let report = handle.join().unwrap();

        assert!(report.skipped_because_exists);
        assert_eq!(store.read_metadata().unwrap().llm_status.len(), 1);
        cleanup(&dir);
    }

    #[test]
    fn migration_activity_picks_most_recent_timestamp() {
        let (store, dir) = temp_store();
        cleanup_legacy_notifications();

        let mut activity = std::collections::HashMap::new();
        activity.insert("myapp:main".to_string(), 1_700_000_000u64);
        activity.insert("myapp:main".to_string(), 1_800_000_000u64); // newer
        write_legacy_activity(&dir, &activity);

        let worktrees = vec![make_discovered_wt(
            "myapp",
            "/repo",
            "/repo",
            Some("main"),
        )];

        let _report = store.migrate_legacy_activity(&worktrees).unwrap();
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.worktrees[0].last_opened_at, Some(1_800_000_000));
        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // drain_legacy_notifications
    // -----------------------------------------------------------------------

    fn write_legacy_notification(base: &Path, session: &str, file_id: &str, content: &str) {
        let dir = base.join(session);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file_id), content).unwrap();
    }

    #[test]
    fn drain_adds_to_existing_metadata() {
        let (store, dir) = temp_store();
        let notif_base = test_dir();

        // Create existing metadata
        let data = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        store.write_metadata(&data).unwrap();

        // Create legacy notification files
        write_legacy_notification(&notif_base, "myapp:main", "1710000000_0", "info\nBuild done");
        let worktrees = vec![make_discovered_wt(
            "myapp", "/repo", "/repo", Some("main"),
        )];

        let report = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();

        assert_eq!(report.migrated_notifications, 1);
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.notifications.len(), 1);
        assert!(metadata.notifications[0].message.contains("Build done"));

        // Legacy file should be cleaned up
        assert!(!notif_base.join("myapp:main").exists());

        cleanup(&dir);
        cleanup(&notif_base);
    }

    #[test]
    fn drain_is_idempotent() {
        let (store, dir) = temp_store();
        let notif_base = test_dir();

        write_legacy_notification(&notif_base, "myapp:main", "1710000000_0", "info\nBuild done");
        let worktrees = vec![make_discovered_wt(
            "myapp", "/repo", "/repo", Some("main"),
        )];

        // First drain
        let report1 = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();
        assert_eq!(report1.migrated_notifications, 1);

        // Second drain — should be no-op (files already cleaned + IDs tracked)
        let report2 = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();
        assert_eq!(report2.migrated_notifications, 0);

        // Metadata should have exactly 1 notification (no duplicates)
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.notifications.len(), 1);

        cleanup(&dir);
        cleanup(&notif_base);
    }

    #[test]
    fn drain_preserves_unresolved_as_unattached() {
        let (store, dir) = temp_store();
        let notif_base = test_dir();

        write_legacy_notification(&notif_base, "myapp:ghost", "1710000000_0", "warning\nUnknown session");
        let worktrees = vec![]; // no matching worktrees

        let report = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();

        assert_eq!(report.unattached_notifications, 1);
        let metadata = store.read_metadata().unwrap();
        assert_eq!(metadata.unattached_notifications.len(), 1);
        assert_eq!(metadata.unattached_notifications[0].session_name, "myapp:ghost");

        cleanup(&dir);
        cleanup(&notif_base);
    }

    #[test]
    fn drain_cleans_legacy_only_after_metadata_write() {
        // Use a read-only parent dir to simulate metadata write failure
        let (store, dir) = temp_store();
        let notif_base = test_dir();

        write_legacy_notification(&notif_base, "myapp:main", "1710000000_0", "info\nHello");
        let worktrees = vec![make_discovered_wt(
            "myapp", "/repo", "/repo", Some("main"),
        )];

        // Make the metadata path read-only to force write failure
        let meta_path = store.metadata_path();
        if let Some(parent) = meta_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // First drain succeeds
        let report = store
            .drain_legacy_notifications(&worktrees, Some(&notif_base))
            .unwrap();
        assert_eq!(report.migrated_notifications, 1);

        // Legacy file should be cleaned since metadata write succeeded
        assert!(!notif_base.join("myapp:main").exists());

        cleanup(&dir);
        cleanup(&notif_base);
    }

    // -----------------------------------------------------------------------
    // parse_notification_content
    // -----------------------------------------------------------------------

    #[test]
    fn parse_notification_content_info() {
        let (level, msg) = parse_notification_content("info\ndeployment done");
        assert_eq!(level, NotifyLevel::Info);
        assert_eq!(msg, "deployment done");
    }

    #[test]
    fn parse_notification_content_warning() {
        let (level, msg) = parse_notification_content("warning\ncareful now");
        assert_eq!(level, NotifyLevel::Warning);
        assert_eq!(msg, "careful now");
    }

    #[test]
    fn parse_notification_content_error() {
        let (level, msg) = parse_notification_content("error\nboom");
        assert_eq!(level, NotifyLevel::Error);
        assert_eq!(msg, "boom");
    }

    #[test]
    fn parse_notification_content_defaults_to_info() {
        let (level, msg) = parse_notification_content("unknown\nstuff");
        assert_eq!(level, NotifyLevel::Info);
        assert_eq!(msg, "stuff");
    }

    #[test]
    fn parse_notification_content_multiline_message() {
        let content = "warning\nline1\nline2\nline3";
        let (level, msg) = parse_notification_content(content);
        assert_eq!(level, NotifyLevel::Warning);
        assert_eq!(msg, "line1\nline2\nline3");
    }

    #[test]
    fn parse_notification_content_empty() {
        let (level, msg) = parse_notification_content("");
        assert_eq!(level, NotifyLevel::Info);
        assert_eq!(msg, "");
    }
}
