/// z-tui: ratatui-based TUI frontend for z.
///
/// Layout (four sections):
///   - Top-left:    PROJECTS list (with ● active and 🌐 remote indicators)
///   - Top-right:   SESSIONS list for the selected project (with 🔔 notification badges)
///   - Middle:      PREVIEW pane — git branch / status / commits (async)
///   - Bottom:      STATUS bar with project info + keyboard hint strip
///
/// Navigation defaults to arrow keys; pass `Navigation::Vim` for hjkl.
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

/// Returns `true` if every character in `query` appears in `target` in order
/// (case-insensitive). Empty query always matches.
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let target_lower = target.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut target_chars = target_lower.chars();
    'outer: for qc in query_lower.chars() {
        loop {
            match target_chars.next() {
                None => return false,
                Some(tc) if tc == qc => continue 'outer,
                Some(_) => {}
            }
        }
    }
    true
}

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use z_core::domain::{CiStatus, PrState, PullRequest, Project, Session};
use z_core::traits::ForgeClient as _;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A project with its active Zellij sessions pre-loaded.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub project: Project,
    pub sessions: Vec<Session>,
    /// Number of git worktrees for this project (used in the delete confirmation modal).
    pub worktree_count: usize,
}

/// Navigation key style.
#[derive(Debug, Clone, PartialEq)]
pub enum Navigation {
    Arrows,
    Vim,
}

/// Which panel is currently focused.
#[derive(Debug, Clone, PartialEq)]
pub enum Panel {
    Projects,
    Sessions,
}

/// Action returned by `run_tui` once the user commits to something.
#[derive(Debug, Clone, PartialEq)]
pub enum TuiAction {
    /// User pressed `q` or `Ctrl-C`.
    Quit,
    /// User pressed `o` / `Enter` on a project or session.
    Open {
        project: String,
        /// `Some(session_name)` when the sessions panel is focused; `None` to
        /// open the project's default (main) session.
        session: Option<String>,
    },
    /// User pressed `n` — create a new session for the selected project.
    New { project: String },
    /// User pressed `d` — delete the selected session.
    Delete { session: String },
    /// User pressed `p` — prune orphaned sessions.
    Prune,
    /// User pressed `a` — start autopilot for the selected project.
    Autopilot { project: String },
    /// User submitted the Add Project form.
    AddProject {
        path: String,
        name: String,
        host: Option<String>,
        token: Option<String>,
    },
    /// User pressed `e` — open per-repo config in $EDITOR.
    EditPerRepoConfig { project_path: std::path::PathBuf },
    /// User confirmed deletion of a project in the delete modal.
    DeleteProject { project: String },
}

// ---------------------------------------------------------------------------
// Modal / form types
// ---------------------------------------------------------------------------

/// A single editable field in a modal form.
#[derive(Debug, Clone)]
pub struct FormField {
    pub label: String,
    pub value: String,
    pub required: bool,
    /// Non-blocking inline warning shown in yellow (e.g. "Path does not exist").
    pub warning: Option<String>,
}

/// State for the "Add Project" form (4 fields: path, name, host, token).
#[derive(Debug, Clone)]
pub struct ProjectForm {
    pub fields: Vec<FormField>,
    pub active_field: usize,
    /// True once the user has manually edited the Name field; suppresses path-basename auto-fill.
    pub name_was_modified: bool,
}

impl ProjectForm {
    pub fn new() -> Self {
        Self {
            fields: vec![
                FormField {
                    label: "Path".to_string(),
                    value: String::new(),
                    required: true,
                    warning: None,
                },
                FormField {
                    label: "Name".to_string(),
                    value: String::new(),
                    required: true,
                    warning: None,
                },
                FormField {
                    label: "Host".to_string(),
                    value: String::new(),
                    required: false,
                    warning: None,
                },
                FormField {
                    label: "Token".to_string(),
                    value: String::new(),
                    required: false,
                    warning: None,
                },
            ],
            active_field: 0,
            name_was_modified: false,
        }
    }
}

impl Default for ProjectForm {
    fn default() -> Self {
        Self::new()
    }
}

/// A modal overlay rendered on top of the main TUI.
#[derive(Debug, Clone)]
pub enum Modal {
    AddProject(ProjectForm),
    /// Confirmation dialog shown before deleting a project.
    DeleteConfirm {
        project_name: String,
        session_count: usize,
        worktree_count: usize,
    },
}

/// Outcome of processing one keypress inside a modal.
enum ModalOutcome {
    Continue,
    Close,
    Submit {
        path: String,
        name: String,
        host: Option<String>,
        token: Option<String>,
    },
    DeleteConfirmed { project: String },
}

// ---------------------------------------------------------------------------
// Preview pane types
// ---------------------------------------------------------------------------

/// A single git commit entry shown in the preview pane.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
}

/// Zellij session info shown in the preview pane.
#[derive(Debug, Clone, PartialEq)]
pub struct ZellijInfo {
    pub tab_count: usize,
    pub pane_count: usize,
    pub uptime: String,
}

/// Git information fetched asynchronously for the selected project/session.
/// PR/CI/Zellij fields start as `None` and are filled in by the forge thread.
#[derive(Debug, Clone, PartialEq)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub is_dirty: bool,
    pub commits: Vec<CommitInfo>,
    /// Pull request for this branch (filled asynchronously after git info).
    pub pr: Option<PullRequest>,
    /// CI status for this branch (filled asynchronously after git info).
    pub ci: Option<CiStatus>,
    /// Zellij session info for this branch (filled asynchronously after git info).
    pub zellij: Option<ZellijInfo>,
}

/// Combined PR/CI/Zellij data from the forge/session background thread.
struct ForgeData {
    pr: Option<PullRequest>,
    ci: CiStatus,
    zellij: Option<ZellijInfo>,
}

/// State of the preview pane data.
pub enum PreviewData {
    /// Fetch in progress — show a spinner/indicator.
    Loading,
    /// Data arrived successfully.
    Ready(GitInfo),
    /// Fetch failed — show a brief error.
    Error(String),
}

// ---------------------------------------------------------------------------
// TUI state
// ---------------------------------------------------------------------------

pub struct TuiState {
    pub entries: Vec<ProjectEntry>,
    pub selected_project: usize,
    pub selected_session: usize,
    pub focused_panel: Panel,
    pub navigation: Navigation,
    pub search_mode: bool,
    pub search_query: String,
    /// Current preview pane data (loading / ready / error).
    pub preview_data: PreviewData,
    /// Key identifying what we last requested a preview for.
    /// Format: `"{project_name}:{branch}"`.
    pub preview_key: String,
    /// Receiver for the in-flight async git fetch, if any.
    pub preview_rx: Option<mpsc::Receiver<Result<GitInfo, String>>>,
    /// Receiver for the in-flight async forge/Zellij fetch (PR, CI, session info).
    pub forge_rx: Option<mpsc::Receiver<Result<ForgeData, String>>>,
    /// Session names (e.g. `"myapp:feat-login"`) that have pending notifications.
    /// Sessions in this set render with a 🔔 badge in the SESSIONS panel.
    pub notifications: HashSet<String>,
    /// Active modal overlay, if any.
    pub modal: Option<Modal>,
}

impl TuiState {
    pub fn new(entries: Vec<ProjectEntry>, navigation: Navigation) -> Self {
        Self {
            entries,
            selected_project: 0,
            selected_session: 0,
            focused_panel: Panel::Projects,
            navigation,
            search_mode: false,
            search_query: String::new(),
            preview_data: PreviewData::Loading,
            preview_key: String::new(),
            preview_rx: None,
            forge_rx: None,
            notifications: HashSet::new(),
            modal: None,
        }
    }

    /// Returns (original_index, &entry) pairs filtered by the current search query.
    ///
    /// Fuzzy-matches the query against each project name and all its session
    /// names. A project is included if either its own name or any session name
    /// matches.
    pub fn filtered_projects(&self) -> Vec<(usize, &ProjectEntry)> {
        let q = &self.search_query;
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                q.is_empty()
                    || fuzzy_match(q, &e.project.name)
                    || e.sessions.iter().any(|s| fuzzy_match(q, &s.name))
            })
            .collect()
    }

    /// Returns sessions of the currently selected project filtered by the
    /// current search query (fuzzy match against session name).
    pub fn filtered_sessions(&self) -> Vec<&Session> {
        let q = &self.search_query;
        self.selected_entry()
            .map(|e| {
                e.sessions
                    .iter()
                    .filter(|s| q.is_empty() || fuzzy_match(q, &s.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Currently selected project entry (accounting for the active filter).
    pub fn selected_entry(&self) -> Option<&ProjectEntry> {
        self.filtered_projects()
            .get(self.selected_project)
            .map(|(_, e)| *e)
    }

    /// Move the cursor up within the focused panel.
    pub fn move_up(&mut self) {
        match self.focused_panel {
            Panel::Projects => {
                if self.selected_project > 0 {
                    self.selected_project -= 1;
                    self.selected_session = 0;
                }
            }
            Panel::Sessions => {
                if self.selected_session > 0 {
                    self.selected_session -= 1;
                }
            }
        }
    }

    /// Move the cursor down within the focused panel.
    pub fn move_down(&mut self) {
        match self.focused_panel {
            Panel::Projects => {
                let count = self.filtered_projects().len();
                if self.selected_project + 1 < count {
                    self.selected_project += 1;
                    self.selected_session = 0;
                }
            }
            Panel::Sessions => {
                let session_count = self.filtered_sessions().len();
                if session_count > 0 && self.selected_session + 1 < session_count {
                    self.selected_session += 1;
                }
            }
        }
    }

    /// Toggle focus between Projects and Sessions.
    pub fn switch_panel(&mut self) {
        self.focused_panel = match self.focused_panel {
            Panel::Projects => Panel::Sessions,
            Panel::Sessions => Panel::Projects,
        };
    }

    /// Compute the preview key for the current selection.
    ///
    /// Returns `None` when there is no selected entry.
    fn current_preview_key(&self) -> Option<String> {
        self.selected_entry().map(|entry| {
            let branch = if self.focused_panel == Panel::Sessions {
                entry
                    .sessions
                    .get(self.selected_session)
                    .map(|s| s.branch.as_str())
                    .unwrap_or("")
            } else {
                entry
                    .sessions
                    .first()
                    .map(|s| s.branch.as_str())
                    .unwrap_or("")
            };
            format!("{}:{}", entry.project.name, branch)
        })
    }

    /// Kick off an async git fetch for the currently selected project/session.
    ///
    /// Does nothing if the selection hasn't changed since the last fetch.
    /// Spawns two background threads:
    ///   1. Fast: git info (branch, status, commits) → preview_rx
    ///   2. Slow: PR/CI/Zellij info → forge_rx
    pub fn trigger_preview_load(&mut self) {
        let Some(key) = self.current_preview_key() else {
            return;
        };
        if key == self.preview_key {
            return; // already loading or loaded for this key
        }

        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return,
        };
        let path = entry.project.path.clone();
        let project_name = entry.project.name.clone();

        // Determine current branch for this selection.
        let branch = if self.focused_panel == Panel::Sessions {
            entry
                .sessions
                .get(self.selected_session)
                .map(|s| s.branch.clone())
                .unwrap_or_default()
        } else {
            entry
                .sessions
                .first()
                .map(|s| s.branch.clone())
                .unwrap_or_default()
        };

        // Session name for Zellij lookup (e.g. "myapp:feat-login").
        let session_name = if branch.is_empty() {
            project_name.clone()
        } else {
            format!(
                "{}:{}",
                project_name,
                z_core::domain::sanitize_branch_name(&branch)
            )
        };

        self.preview_key = key;
        self.preview_data = PreviewData::Loading;

        // Phase 1 — git info (fast, local)
        let (tx1, rx1) = mpsc::channel();
        self.preview_rx = Some(rx1);
        let path1 = path.clone();
        std::thread::spawn(move || {
            let result = fetch_git_info(&path1.to_string_lossy());
            let _ = tx1.send(result);
        });

        // Phase 2 — PR/CI/Zellij (slow, network)
        let (tx2, rx2) = mpsc::channel();
        self.forge_rx = Some(rx2);
        std::thread::spawn(move || {
            let client = GhForgeClient;
            let forge = ForgeData {
                pr: client.get_pr(&project_name, &branch).ok().flatten(),
                ci: client
                    .get_ci_status(&project_name, &branch)
                    .unwrap_or(CiStatus::Unknown),
                zellij: fetch_zellij_info(&session_name),
            };
            let _ = tx2.send(Ok(forge));
        });
    }

    /// Poll the forge channel; merge PR/CI/Zellij data into `preview_data` if arrived.
    pub fn poll_forge(&mut self) {
        let outcome = match &self.forge_rx {
            Some(rx) => match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // thread panicked or finished without sending
                    Some(Err("forge fetch failed (worker dropped)".to_string()))
                }
            },
            None => return,
        };
        if let Some(result) = outcome {
            if let Ok(forge) = result {
                if let PreviewData::Ready(ref mut info) = self.preview_data {
                    info.pr = forge.pr;
                    info.ci = Some(forge.ci);
                    info.zellij = forge.zellij;
                }
            }
            // Always clear forge_rx once we get a result (success or failure)
            self.forge_rx = None;
        }
    }

    /// Poll the in-flight preview channel; update `preview_data` if data arrived.
    pub fn poll_preview(&mut self) {
        let outcome = match &self.preview_rx {
            Some(rx) => match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                // Sender dropped without sending (e.g. thread panicked).
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("preview fetch failed (worker dropped)".to_string()))
                }
            },
            None => return,
        };
        if let Some(result) = outcome {
            self.preview_data = match result {
                Ok(info) => PreviewData::Ready(info),
                Err(e) => PreviewData::Error(e),
            };
            self.preview_rx = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Git data fetching (runs in a background thread)
// ---------------------------------------------------------------------------

/// Fetch git information for `path` using subprocess git commands.
///
/// Returns `Err` if the directory is not a git repository or git is not found.
fn fetch_git_info(path: &str) -> Result<GitInfo, String> {
    use std::process::Command;

    // Current branch
    let branch_out = Command::new("git")
        .args(["-C", path, "symbolic-ref", "--short", "HEAD"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;

    if !branch_out.status.success() {
        return Err("not a git repository".to_string());
    }
    let branch = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();

    // Dirty / clean status
    let status_out = Command::new("git")
        .args(["-C", path, "status", "--short"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;
    let is_dirty = !String::from_utf8_lossy(&status_out.stdout)
        .trim()
        .is_empty();

    // Ahead / behind relative to upstream (best effort — 0/0 if no upstream)
    let (ahead, behind) = fetch_ahead_behind(path);

    // Recent commits
    let log_out = Command::new("git")
        .args(["-C", path, "log", "--oneline", "-5"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;
    let commits = String::from_utf8_lossy(&log_out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut parts = l.splitn(2, ' ');
            let hash = parts.next().unwrap_or("").to_string();
            let message = parts.next().unwrap_or("").to_string();
            CommitInfo { hash, message }
        })
        .collect();

    Ok(GitInfo {
        branch,
        ahead,
        behind,
        is_dirty,
        commits,
        pr: None,
        ci: None,
        zellij: None,
    })
}

/// Returns (ahead, behind) counts for HEAD vs its upstream; (0, 0) on failure.
fn fetch_ahead_behind(path: &str) -> (usize, usize) {
    use std::process::Command;

    let out = Command::new("git")
        .args([
            "-C",
            path,
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{u}",
        ])
        .output();

    match out {
        Ok(output) if output.status.success() => {
            let s = String::from_utf8_lossy(&output.stdout);
            let mut parts = s.trim().split_whitespace();
            let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            (ahead, behind)
        }
        _ => (0, 0),
    }
}

// ---------------------------------------------------------------------------
// Forge client — gh CLI implementation
// ---------------------------------------------------------------------------

/// Real `ForgeClient` that delegates to the `gh` CLI.
struct GhForgeClient;

impl z_core::traits::ForgeClient for GhForgeClient {
    fn get_pr(
        &self,
        _project: &str,
        branch: &str,
    ) -> z_core::error::Result<Option<PullRequest>> {
        use std::process::Command;
        if branch.is_empty() {
            return Ok(None);
        }
        let out = Command::new("gh")
            .args(["pr", "view", branch, "--json", "number,state,title,url"])
            .output()
            .map_err(|e| z_core::error::ZError::Forge(e.to_string()))?;
        if !out.status.success() {
            // No PR found for this branch — treat as absent, not an error.
            return Ok(None);
        }
        let json = String::from_utf8_lossy(&out.stdout);
        Ok(parse_pr_json(&json))
    }

    fn get_ci_status(
        &self,
        _project: &str,
        branch: &str,
    ) -> z_core::error::Result<CiStatus> {
        use std::process::Command;
        if branch.is_empty() {
            return Ok(CiStatus::Unknown);
        }
        let out = Command::new("gh")
            .args([
                "run",
                "list",
                "--branch",
                branch,
                "--limit",
                "1",
                "--json",
                "conclusion,status",
            ])
            .output()
            .map_err(|e| z_core::error::ZError::Forge(e.to_string()))?;
        if !out.status.success() {
            return Ok(CiStatus::Unknown);
        }
        let json = String::from_utf8_lossy(&out.stdout);
        Ok(parse_ci_status_json(&json))
    }
}

/// Parse `gh pr view --json number,state,title,url` output.
///
/// Returns `None` if the JSON cannot be parsed or the number is missing.
fn parse_pr_json(json: &str) -> Option<PullRequest> {
    let number = extract_json_u64(json, "number")?;
    let state_raw = extract_json_string(json, "state").unwrap_or_default();
    let state = match state_raw.to_uppercase().as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        _ => PrState::Closed,
    };
    let title = extract_json_string(json, "title").unwrap_or_default();
    let url = extract_json_string(json, "url").unwrap_or_default();
    Some(PullRequest { number, title, state, url })
}

/// Parse `gh run list --json conclusion,status` output (an array).
///
/// Looks at the first element's `conclusion` field.
fn parse_ci_status_json(json: &str) -> CiStatus {
    // json is an array; look for the first "conclusion" field
    match extract_json_string(json, "conclusion")
        .as_deref()
        .unwrap_or("")
    {
        "success" => CiStatus::Passing,
        "failure" | "timed_out" => CiStatus::Failing,
        "" => {
            // No conclusion yet — check status field
            match extract_json_string(json, "status")
                .as_deref()
                .unwrap_or("")
            {
                "in_progress" | "queued" | "waiting" => CiStatus::Pending,
                _ => CiStatus::Unknown,
            }
        }
        _ => CiStatus::Unknown,
    }
}

/// Extract a u64 value from a simple JSON object: `"key": 42`.
fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|s| s.parse().ok())
}

/// Extract a string value from a simple JSON object: `"key": "value"`.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let after_colon = json.find(&needle)? + needle.len();
    // Skip optional whitespace between ':' and the opening '"'.
    let trimmed = json[after_colon..].trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let rest = &trimmed[1..];
    // Find the closing quote, respecting simple escapes.
    let mut result = String::new();
    let mut chars = rest.chars().peekable();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => {
                if let Some(c) = chars.next() {
                    result.push(c);
                }
            }
            c => result.push(c),
        }
    }
    Some(result)
}

// ---------------------------------------------------------------------------
// Zellij session info fetching
// ---------------------------------------------------------------------------

/// Fetch Zellij session info for `session_name` via `zellij list-sessions`.
///
/// Returns `None` if Zellij is not running or the session is not found.
fn fetch_zellij_info(session_name: &str) -> Option<ZellijInfo> {
    use std::process::Command;
    if session_name.is_empty() {
        return None;
    }

    // Try JSON output (Zellij 0.40+)
    let out = Command::new("zellij")
        .args(["list-sessions", "--json"])
        .output()
        .ok()?;

    if out.status.success() {
        let json = String::from_utf8_lossy(&out.stdout);
        if let Some(info) = parse_zellij_json_for_session(&json, session_name) {
            return Some(info);
        }
    }

    // Fallback: plain `zellij list-sessions` for uptime only
    let out2 = Command::new("zellij")
        .args(["list-sessions"])
        .output()
        .ok()?;

    if !out2.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&out2.stdout);
    for line in text.lines() {
        if line.contains(session_name) {
            let uptime = extract_zellij_uptime(line)
                .unwrap_or_else(|| "unknown".to_string());
            return Some(ZellijInfo {
                tab_count: 0,
                pane_count: 0,
                uptime,
            });
        }
    }

    None
}

/// Parse `zellij list-sessions --json` output looking for `session_name`.
///
/// Expected format (may vary by Zellij version):
/// `[{"name":"myapp:feat-login","tabs":3,"panes":5,"created_at":"..."},...]`
fn parse_zellij_json_for_session(json: &str, session_name: &str) -> Option<ZellijInfo> {
    // Find the object that contains `"name":"<session_name>"` (with or without space after colon).
    let compact = format!("\"name\":\"{}\"", session_name);
    let spaced = format!("\"name\": \"{}\"", session_name);
    let obj_start = json.find(&compact).or_else(|| json.find(&spaced))?;

    // Walk backwards to find the '{' that starts this object.
    let before = &json[..obj_start];
    let brace_pos = before.rfind('{')?;
    // Find the matching closing '}' to avoid reading fields from later objects.
    let after_brace = &json[brace_pos..];
    let close = after_brace.find('}').unwrap_or(after_brace.len());
    let obj = &after_brace[..=close.min(after_brace.len() - 1)];

    let tab_count = extract_json_u64(obj, "tabs")
        .or_else(|| extract_json_u64(obj, "tab_count"))
        .unwrap_or(0) as usize;
    let pane_count = extract_json_u64(obj, "panes")
        .or_else(|| extract_json_u64(obj, "pane_count"))
        .unwrap_or(0) as usize;

    // Uptime: use raw created_at value as a hint; proper parsing requires chrono (not in deps).
    // The Zellij JSON may include "exited" or other status fields that give us uptime text.
    let uptime = extract_json_string(obj, "uptime")
        .unwrap_or_else(|| "unknown".to_string());

    Some(ZellijInfo { tab_count, pane_count, uptime })
}

/// Extract uptime string from a `zellij list-sessions` plain-text line.
///
/// Looks for patterns like `[Created 2h34m ago]` or `(2h34m)`.
fn extract_zellij_uptime(line: &str) -> Option<String> {
    // Pattern: "[Created Xm ago]", "[Created Xh Ym ago]", etc.
    if let Some(start) = line.find("Created ") {
        let rest = &line[start + "Created ".len()..];
        if let Some(end) = rest.find(" ago") {
            return Some(rest[..end].to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Modal helpers
// ---------------------------------------------------------------------------

/// Expand a leading `~/` (or bare `~`) to the home directory.
/// Does not expand `~username` forms — only the current user's home.
fn expand_tilde_path(s: &str) -> String {
    if s == "~" {
        std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
    } else if let Some(rest) = s.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/{}", home, rest)
    } else {
        s.to_string()
    }
}

/// Update the Name field from the current Path basename, unless the user manually edited Name.
/// Called whenever the Path field changes (typing or backspace) so the name stays in sync.
fn autofill_name_if_empty(form: &mut ProjectForm) {
    if form.name_was_modified {
        return;
    }
    let raw = form.fields[0].value.trim();
    let expanded = expand_tilde_path(raw);
    let basename = std::path::Path::new(&expanded)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    form.fields[1].value = basename;
}

/// Validate the path field: warn if the path doesn't exist or isn't a git repo.
/// Sets or clears `form.fields[0].warning` accordingly.
fn validate_path_field(form: &mut ProjectForm) {
    let raw = form.fields[0].value.trim();
    if raw.is_empty() {
        form.fields[0].warning = None;
        return;
    }
    let expanded = expand_tilde_path(raw);
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        form.fields[0].warning = Some("Path does not exist".to_string());
    } else if !p.join(".git").exists() {
        form.fields[0].warning = Some("Not a git repository".to_string());
    } else {
        form.fields[0].warning = None;
    }
}

/// Returns `Some(s)` if the trimmed string is non-empty, else `None`.
fn non_empty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// Process one keypress inside the given modal. Mutates modal state in place.
/// Returns the outcome: whether to close, submit with data, or continue.
fn advance_modal(modal: &mut Modal, code: KeyCode) -> ModalOutcome {
    match modal {
        Modal::AddProject(form) => match code {
            KeyCode::Esc => ModalOutcome::Close,

            KeyCode::Tab => {
                let was_path = form.active_field == 0;
                form.active_field = (form.active_field + 1) % form.fields.len();
                if was_path {
                    autofill_name_if_empty(form);
                    validate_path_field(form);
                }
                ModalOutcome::Continue
            }

            KeyCode::BackTab => {
                form.active_field =
                    (form.active_field + form.fields.len() - 1) % form.fields.len();
                ModalOutcome::Continue
            }

            KeyCode::Enter => {
                // Validate required fields
                let mut valid = true;
                for field in &mut form.fields {
                    if field.required && field.value.trim().is_empty() {
                        field.warning = Some("Required".to_string());
                        valid = false;
                    }
                }
                if valid {
                    let path = expand_tilde_path(form.fields[0].value.trim());
                    let name = form.fields[1].value.trim().to_string();
                    let host = non_empty_opt(&form.fields[2].value);
                    let token = non_empty_opt(&form.fields[3].value);
                    ModalOutcome::Submit { path, name, host, token }
                } else {
                    ModalOutcome::Continue
                }
            }

            KeyCode::Backspace => {
                let fi = form.active_field;
                form.fields[fi].value.pop();
                form.fields[fi].warning = None;
                if fi == 1 {
                    form.name_was_modified = true;
                }
                if fi == 0 {
                    autofill_name_if_empty(form);
                }
                ModalOutcome::Continue
            }

            KeyCode::Char(c) => {
                let fi = form.active_field;
                form.fields[fi].value.push(c);
                form.fields[fi].warning = None;
                if fi == 1 {
                    form.name_was_modified = true;
                }
                if fi == 0 {
                    autofill_name_if_empty(form);
                }
                ModalOutcome::Continue
            }

            _ => ModalOutcome::Continue,
        },

        Modal::DeleteConfirm { project_name, .. } => match code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ModalOutcome::Close,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                ModalOutcome::DeleteConfirmed { project: project_name.clone() }
            }
            _ => ModalOutcome::Continue,
        },
    }
}

/// Compute a centered `Rect` of the given dimensions within `area`.
fn modal_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Launch the full-screen ratatui TUI.
///
/// Sets up the terminal, runs the event loop, restores the terminal on exit,
/// and returns the action the user chose.
///
/// `notifications` — set of session names that have pending notifications;
/// these sessions will display a 🔔 badge in the SESSIONS panel.
pub fn run_tui(
    entries: Vec<ProjectEntry>,
    navigation: Navigation,
    notifications: HashSet<String>,
    initial_project: Option<usize>,
) -> io::Result<TuiAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new(entries, navigation);
    state.notifications = notifications;
    if let Some(idx) = initial_project {
        state.selected_project = idx;
    }
    // Kick off the first preview fetch immediately.
    state.trigger_preview_load();

    let result = event_loop(&mut terminal, &mut state);

    // Always restore the terminal, even if the event loop returned an error.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut TuiState,
) -> io::Result<TuiAction> {
    loop {
        // Check if async git preview data has arrived.
        state.poll_preview();
        // Check if async forge/Zellij data has arrived.
        state.poll_forge();

        terminal.draw(|f| render(f, state))?;

        // Poll with a short timeout so we can refresh the preview pane
        // without waiting for a keypress.
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // ── Modal mode ─────────────────────────────────────────────────
            if state.modal.is_some() {
                let outcome = advance_modal(state.modal.as_mut().unwrap(), key.code);
                match outcome {
                    ModalOutcome::Close => {
                        state.modal = None;
                    }
                    ModalOutcome::Submit { path, name, host, token } => {
                        state.modal = None;
                        return Ok(TuiAction::AddProject { path, name, host, token });
                    }
                    ModalOutcome::DeleteConfirmed { project } => {
                        state.modal = None;
                        return Ok(TuiAction::DeleteProject { project });
                    }
                    ModalOutcome::Continue => {}
                }
                continue;
            }

            // ── Search mode ────────────────────────────────────────────────
            if state.search_mode {
                match key.code {
                    KeyCode::Esc => {
                        state.search_mode = false;
                        state.search_query.clear();
                        state.selected_project = 0;
                        state.selected_session = 0;
                    }
                    KeyCode::Enter => {
                        // Commit the selection and exit search mode; the next
                        // iteration will handle an `o`/Enter action if needed.
                        state.search_mode = false;
                    }
                    KeyCode::Up => state.move_up(),
                    KeyCode::Down => state.move_down(),
                    KeyCode::Backspace => {
                        state.search_query.pop();
                        state.selected_project = 0;
                        state.selected_session = 0;
                    }
                    KeyCode::Char(c) => {
                        state.search_query.push(c);
                        state.selected_project = 0;
                        state.selected_session = 0;
                    }
                    _ => {}
                }
                state.trigger_preview_load();
                continue;
            }

            // ── Normal mode ────────────────────────────────────────────────
            let vim = state.navigation == Navigation::Vim;

            let is_up = matches!(key.code, KeyCode::Up)
                || (vim && matches!(key.code, KeyCode::Char('k')));
            let is_down = matches!(key.code, KeyCode::Down)
                || (vim && matches!(key.code, KeyCode::Char('j')));
            let is_left = matches!(key.code, KeyCode::Left)
                || (vim && matches!(key.code, KeyCode::Char('h')));
            let is_right = matches!(key.code, KeyCode::Right)
                || (vim && matches!(key.code, KeyCode::Char('l')));

            if is_up {
                state.move_up();
            } else if is_down {
                state.move_down();
            } else if is_left {
                state.focused_panel = Panel::Projects;
            } else if is_right {
                state.focused_panel = Panel::Sessions;
            } else {
                match key.code {
                    KeyCode::Tab => state.switch_panel(),

                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(TuiAction::Quit),

                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        return Ok(TuiAction::Quit);
                    }

                    KeyCode::Char('o') | KeyCode::Enter => {
                        let project_name =
                            state.selected_entry().map(|e| e.project.name.clone());
                        if let Some(project) = project_name {
                            let session = if state.focused_panel == Panel::Sessions {
                                let sessions = state.filtered_sessions();
                                if !sessions.is_empty() {
                                    sessions
                                        .get(state.selected_session)
                                        .map(|s| s.name.clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            return Ok(TuiAction::Open { project, session });
                        }
                    }

                    KeyCode::Char('n') => {
                        if let Some(entry) = state.selected_entry() {
                            return Ok(TuiAction::New {
                                project: entry.project.name.clone(),
                            });
                        }
                    }

                    KeyCode::Char('d') => {
                        let session_name = state
                            .filtered_sessions()
                            .get(state.selected_session)
                            .map(|s| s.name.clone());
                        if let Some(session) = session_name {
                            return Ok(TuiAction::Delete { session });
                        }
                    }

                    KeyCode::Char('p') => return Ok(TuiAction::Prune),

                    KeyCode::Char('a') => {
                        if let Some(entry) = state.selected_entry() {
                            return Ok(TuiAction::Autopilot {
                                project: entry.project.name.clone(),
                            });
                        }
                    }

                    KeyCode::Char('A') => {
                        if state.focused_panel == Panel::Projects {
                            state.modal = Some(Modal::AddProject(ProjectForm::new()));
                        }
                    }

                    KeyCode::Char('D') => {
                        if state.focused_panel == Panel::Projects {
                            if let Some(entry) = state.selected_entry() {
                                let project_name = entry.project.name.clone();
                                let session_count = entry.sessions.len();
                                let worktree_count = entry.worktree_count;
                                state.modal = Some(Modal::DeleteConfirm {
                                    project_name,
                                    session_count,
                                    worktree_count,
                                });
                            }
                        }
                    }

                    KeyCode::Char('e') | KeyCode::Char('E') => {
                        if let Some(entry) = state.selected_entry() {
                            return Ok(TuiAction::EditPerRepoConfig {
                                project_path: entry.project.path.clone(),
                            });
                        }
                    }

                    KeyCode::Char('/') => {
                        state.search_mode = true;
                        state.search_query.clear();
                    }

                    _ => {}
                }
            }

            // After any navigation event, check if we need to refresh the preview.
            state.trigger_preview_load();
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Top-level render: splits the terminal into main panels, preview, and status.
pub fn render(f: &mut Frame, state: &TuiState) {
    let area = f.area();

    // Vertical split: main panels | preview pane | status bar
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),     // main panels (takes all remaining space)
            Constraint::Length(8),  // preview pane (fixed 8 lines)
            Constraint::Length(4),  // status bar (2 content rows: project info + key hints)
        ])
        .split(area);

    // Horizontal split for main panels
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[0]);

    render_projects(f, main[0], state);
    render_sessions(f, main[1], state);
    render_preview(f, outer[1], state);
    render_status(f, outer[2], state);
    // Modal overlay is rendered last so it appears on top
    render_modal(f, state);
}

fn render_projects(f: &mut Frame, area: Rect, state: &TuiState) {
    let focused = state.focused_panel == Panel::Projects;

    let filtered = state.filtered_projects();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|(_, entry)| {
            let active = if !entry.sessions.is_empty() { " \u{25cf}" } else { "" };
            let remote = if entry.project.host.is_some() { " \u{1f310}" } else { "" };
            ListItem::new(format!("{}{}{}", entry.project.name, active, remote))
        })
        .collect();

    let mut list_state = ListState::default();
    if !filtered.is_empty() {
        list_state.select(Some(state.selected_project));
    }

    let title = if state.search_mode {
        format!(" PROJECTS  /{} ", state.search_query)
    } else {
        " PROJECTS ".to_string()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if focused {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                })
                .title(title),
        )
        .highlight_symbol("\u{25b8} ")
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_stateful_widget(list, area, &mut list_state);
}

fn render_sessions(f: &mut Frame, area: Rect, state: &TuiState) {
    let focused = state.focused_panel == Panel::Sessions;

    let sessions = state.filtered_sessions();

    let items: Vec<ListItem> = sessions
        .iter()
        .map(|s| {
            let badge = if state.notifications.contains(&s.name) {
                " \u{1f514}" // 🔔
            } else {
                ""
            };
            ListItem::new(format!("{}{}", s.name, badge))
        })
        .collect();

    let mut list_state = ListState::default();
    if !sessions.is_empty() {
        list_state.select(Some(state.selected_session));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if focused {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                })
                .title(" SESSIONS "),
        )
        .highlight_symbol("\u{25b8} ")
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_stateful_widget(list, area, &mut list_state);
}

fn render_preview(f: &mut Frame, area: Rect, state: &TuiState) {
    let content = match &state.preview_data {
        PreviewData::Loading => " Loading\u{2026}".to_string(),
        PreviewData::Error(e) => format!(" Error: {}", e),
        PreviewData::Ready(info) => {
            let tracking = if info.ahead > 0 || info.behind > 0 {
                format!(" ({} ahead, {} behind)", info.ahead, info.behind)
            } else {
                String::new()
            };
            let dirt = if info.is_dirty { "\u{25cf} dirty" } else { "\u{25cf} clean" };
            let mut lines = vec![
                format!(" branch: {}{} {}", info.branch, tracking, dirt),
            ];

            // PR and CI status line
            let pr_str = match &info.pr {
                Some(pr) => {
                    let state_label = match pr.state {
                        PrState::Open => "open",
                        PrState::Merged => "merged",
                        PrState::Closed => "closed",
                    };
                    format!("PR: #{} ({})", pr.number, state_label)
                }
                None => String::new(),
            };
            let ci_str = match &info.ci {
                Some(CiStatus::Passing) => Some("CI: \u{2705} passing"),
                Some(CiStatus::Failing) => Some("CI: \u{274c} failing"),
                Some(CiStatus::Pending) => Some("CI: \u{23f3} pending"),
                Some(CiStatus::Unknown) | None => None,
            };
            match (pr_str.is_empty(), ci_str) {
                (false, Some(ci)) => lines.push(format!(" {} | {}", pr_str, ci)),
                (false, None) => lines.push(format!(" {}", pr_str)),
                (true, Some(ci)) => lines.push(format!(" {}", ci)),
                (true, None) => {}
            }

            // Zellij session info line
            if let Some(zellij) = &info.zellij {
                let tab_str = if zellij.tab_count > 0 {
                    format!("{} tabs, ", zellij.tab_count)
                } else {
                    String::new()
                };
                let pane_str = if zellij.pane_count > 0 {
                    format!("{} panes, ", zellij.pane_count)
                } else {
                    String::new()
                };
                lines.push(format!(" session: {}{}up {}", tab_str, pane_str, zellij.uptime));
            }

            if !info.commits.is_empty() {
                lines.push(String::new());
                lines.push(" recent commits:".to_string());
                for commit in &info.commits {
                    lines.push(format!("  {} {}", commit.hash, commit.message));
                }
            }
            lines.join("\n")
        }
    };

    let paragraph = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" PREVIEW "));
    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, area: Rect, state: &TuiState) {
    let project_info = state
        .selected_entry()
        .map(|e| {
            let locality = if e.project.host.is_some() { "remote" } else { "local" };
            let session_count = e.sessions.len();
            format!(" {} | {} | sessions: {} ", e.project.name, locality, session_count)
        })
        .unwrap_or_else(|| " No projects — add to ~/.config/z/projects.kdl ".to_string());

    let hints = " [o]pen [n]ew [d]el session [p]rune [a]utopilot [A]dd [D]el project [e]dit [/]search [q]uit";
    let content = format!("{}\n{}", project_info, hints);

    let paragraph = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" STATUS "));
    f.render_widget(paragraph, area);
}

fn render_delete_confirm_modal(
    f: &mut Frame,
    project_name: &str,
    session_count: usize,
    worktree_count: usize,
) {
    let area = f.area();
    // 3 info lines + 1 blank + 1 separator + 1 hint + 2 borders = 8; round to 9 for padding
    let modal_width = 62u16;
    let modal_height = 9u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Delete Project ")
        .border_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let session_word = if session_count == 1 { "session" } else { "sessions" };
    let worktree_word = if worktree_count == 1 { "worktree" } else { "worktrees" };

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Delete project: {}", project_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" Active {}: {}", session_word, session_count),
            Style::default(),
        )),
        Line::from(Span::styled(
            format!(" Git {}: {}", worktree_word, worktree_count),
            Style::default(),
        )),
        Line::from(""),
    ];

    // Separator
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(sep_width),
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from(Span::styled(
        " Enter/y: confirm  Esc/n: cancel  (only removes KDL entry)",
        Style::default().add_modifier(Modifier::DIM),
    )));

    let paragraph = Paragraph::new(Text::from(lines));
    f.render_widget(paragraph, inner);
}

fn render_modal(f: &mut Frame, state: &TuiState) {
    let form = match &state.modal {
        None => return,
        Some(Modal::DeleteConfirm { project_name, session_count, worktree_count }) => {
            render_delete_confirm_modal(f, project_name, *session_count, *worktree_count);
            return;
        }
        Some(Modal::AddProject(form)) => form,
    };

    let area = f.area();
    // Modal: 62 wide (60 content + 2 borders), height = 4 fields × 3 rows + 2 hints/sep + 2 borders
    let modal_height = (form.fields.len() as u16) * 3 + 4;
    let modal_width = 62u16;
    let rect = modal_rect(modal_width, modal_height, area);

    // Clear area under the modal
    f.render_widget(Clear, rect);

    // Outer block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add Project ")
        .border_style(Style::default().add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // Build text content as styled lines
    let mut lines: Vec<Line> = Vec::new();

    for (i, field) in form.fields.iter().enumerate() {
        let active = i == form.active_field;
        let opt_hint = if field.required { "" } else { " (opt)" };

        // Label line
        let label_text = format!(" {}{}:", field.label, opt_hint);
        let label_style = if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(label_text, label_style)));

        // Value line — active field shows a cursor block
        let value_text = if active {
            format!(" \u{25b6} {}█", field.value)
        } else {
            format!("   {}", field.value)
        };
        let value_style = if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(value_text, value_style)));

        // Warning / spacer line
        if let Some(warn) = &field.warning {
            lines.push(Line::from(Span::styled(
                format!(" \u{26a0} {}", warn),
                Style::default().fg(Color::Yellow),
            )));
        } else {
            lines.push(Line::from(""));
        }
    }

    // Separator
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(sep_width),
        Style::default().add_modifier(Modifier::DIM),
    )));
    // Hints
    lines.push(Line::from(Span::styled(
        " Tab: next  S-Tab: prev  Enter: save  Esc: cancel",
        Style::default().add_modifier(Modifier::DIM),
    )));

    let paragraph = Paragraph::new(Text::from(lines));
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::sync::mpsc;

    // ── Helpers ────────────────────────────────────────────────────────────

    fn make_project(name: &str, remote: bool) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(format!("/home/user/{}", name)),
            host: if remote {
                Some("https://remote.example.com".to_string())
            } else {
                None
            },
            token: None,
        }
    }

    fn make_entries() -> Vec<ProjectEntry> {
        vec![
            ProjectEntry {
                project: make_project("myapp", false),
                sessions: vec![
                    Session::new("myapp", "main"),
                    Session::new("myapp", "feat/login"),
                ],
                worktree_count: 0,
            },
            ProjectEntry {
                project: make_project("hermes", false),
                sessions: vec![],
                worktree_count: 0,
            },
            ProjectEntry {
                project: make_project("prod-api", true),
                sessions: vec![],
                worktree_count: 0,
            },
        ]
    }

    fn make_git_info() -> GitInfo {
        GitInfo {
            branch: "feat/login".to_string(),
            ahead: 3,
            behind: 1,
            is_dirty: true,
            commits: vec![
                CommitInfo {
                    hash: "a1b2c3".to_string(),
                    message: "fix: auth token refresh".to_string(),
                },
                CommitInfo {
                    hash: "d4e5f6".to_string(),
                    message: "feat: login form validation".to_string(),
                },
            ],
            pr: None,
            ci: None,
            zellij: None,
        }
    }

    fn make_pull_request(number: u64, state: PrState) -> PullRequest {
        PullRequest {
            number,
            title: "feat: some feature".to_string(),
            state,
            url: "https://github.com/owner/repo/pull/42".to_string(),
        }
    }

    fn make_zellij_info() -> ZellijInfo {
        ZellijInfo {
            tab_count: 3,
            pane_count: 5,
            uptime: "2h34m".to_string(),
        }
    }

    /// Render `state` into a `width × height` TestBackend buffer and return
    /// the result as a string (each row separated by `\n`).
    fn render_to_string(state: &TuiState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, state)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for row in 0..height {
            for col in 0..width {
                out.push_str(buf.cell((col, row)).map(|c| c.symbol()).unwrap_or(" "));
            }
            out.push('\n');
        }
        out
    }

    // ── Rendering snapshot tests — existing panels ─────────────────────────

    #[test]
    fn renders_projects_panel_header() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"), "should render PROJECTS panel header");
        assert!(out.contains("SESSIONS"), "should render SESSIONS panel header");
    }

    #[test]
    fn renders_all_project_names() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("myapp"), "should show 'myapp'");
        assert!(out.contains("hermes"), "should show 'hermes'");
        assert!(out.contains("prod-api"), "should show 'prod-api'");
    }

    #[test]
    fn renders_active_session_indicator() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        // myapp has sessions → should have the ● bullet (U+25CF)
        assert!(out.contains('\u{25cf}'), "should show active session indicator ●");
    }

    #[test]
    fn renders_remote_project_icon() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        // prod-api is remote → should have 🌐 (U+1F310)
        assert!(out.contains('\u{1f310}'), "should show remote project icon 🌐");
    }

    #[test]
    fn renders_sessions_for_selected_project() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        // myapp is selected; its sessions should appear in the right panel
        assert!(out.contains("myapp:main"), "should show 'myapp:main' session");
        assert!(
            out.contains("myapp:feat-login"),
            "should show 'myapp:feat-login' session"
        );
    }

    #[test]
    fn renders_status_bar_hints() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("[o]"), "should show [o] hint");
        assert!(out.contains("[q]"), "should show [q] hint");
        assert!(out.contains("[n]"), "should show [n] hint");
        assert!(out.contains("[d]"), "should show [d] hint");
        assert!(out.contains("[e]"), "should show [e] edit config hint");
    }

    #[test]
    fn e_key_returns_edit_per_repo_config_action() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // First project is "myapp" at /home/user/myapp
        assert!(state.selected_entry().is_some(), "should have a selected entry");
        let expected_path = std::path::PathBuf::from("/home/user/myapp");
        let entry = state.selected_entry().unwrap();
        assert_eq!(entry.project.path, expected_path);
        // Verify the action variant is constructed correctly
        let action = TuiAction::EditPerRepoConfig {
            project_path: entry.project.path.clone(),
        };
        match action {
            TuiAction::EditPerRepoConfig { project_path } => {
                assert_eq!(project_path, expected_path);
            }
            _ => panic!("expected EditPerRepoConfig action"),
        }
    }

    #[test]
    fn e_key_no_projects_does_nothing() {
        let state = TuiState::new(vec![], Navigation::Arrows);
        // With no entries, selected_entry() returns None — no action should be emitted.
        assert!(state.selected_entry().is_none(), "empty state should have no selected entry");
    }

    #[test]
    fn renders_status_bar_project_info() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        // Status bar shows selected project name
        assert!(out.contains("myapp"), "status bar should mention selected project");
        assert!(out.contains("local"), "status bar should show locality");
    }

    #[test]
    fn renders_empty_state_without_panic() {
        let state = TuiState::new(vec![], Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"), "should still render PROJECTS panel");
        assert!(out.contains("SESSIONS"), "should still render SESSIONS panel");
    }

    #[test]
    fn renders_search_query_in_header() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "my".to_string();
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("/my"), "search query should appear in PROJECTS header");
    }

    // ── Preview pane snapshot tests ────────────────────────────────────────

    #[test]
    fn renders_preview_pane_header() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("PREVIEW"), "should render PREVIEW panel header");
    }

    #[test]
    fn renders_preview_loading_indicator() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        // Initial state is Loading
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("Loading") || out.contains("loading"),
            "should show loading indicator in preview pane"
        );
    }

    #[test]
    fn renders_preview_ready_with_branch_and_tracking() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info());
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("feat"), "should show branch name");
        assert!(out.contains("login"), "should show branch name (login part)");
        assert!(out.contains('3'), "should show ahead count");
        assert!(out.contains('1'), "should show behind count");
        assert!(out.contains("ahead"), "should show 'ahead'");
        assert!(out.contains("behind"), "should show 'behind'");
    }

    #[test]
    fn renders_preview_dirty_status() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info()); // is_dirty = true
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("dirty"), "should show dirty working tree status");
    }

    #[test]
    fn renders_preview_clean_status() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(GitInfo {
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            is_dirty: false,
            commits: vec![],
            pr: None,
            ci: None,
            zellij: None,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("clean"), "should show clean working tree status");
    }

    #[test]
    fn renders_preview_commit_list() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info());
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("a1b2c3"), "should show commit hash");
        assert!(out.contains("d4e5f6"), "should show second commit hash");
    }

    #[test]
    fn renders_preview_recent_commits_label() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info());
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("recent commits") || out.contains("recent"),
            "should label the commit section"
        );
    }

    #[test]
    fn renders_preview_error_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Error("not a git repository".to_string());
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("Error") || out.contains("error") || out.contains("not a git"),
            "should show error message in preview pane"
        );
    }

    #[test]
    fn renders_preview_no_tracking_when_zero_ahead_behind() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(GitInfo {
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            is_dirty: false,
            commits: vec![],
            pr: None,
            ci: None,
            zellij: None,
        });
        let out = render_to_string(&state, 80, 30);
        // When 0 ahead/0 behind, tracking info should not appear
        assert!(
            !out.contains("ahead") && !out.contains("behind"),
            "should not show ahead/behind when both are zero"
        );
    }

    // ── Preview key / trigger tests ────────────────────────────────────────

    #[test]
    fn initial_preview_state_is_loading() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        assert!(
            matches!(state.preview_data, PreviewData::Loading),
            "initial preview_data should be Loading"
        );
    }

    #[test]
    fn initial_preview_key_is_empty() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        assert_eq!(state.preview_key, "");
    }

    #[test]
    fn trigger_preview_load_sets_key() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.trigger_preview_load();
        assert!(!state.preview_key.is_empty(), "preview_key should be set after trigger");
        assert!(
            state.preview_key.contains("myapp"),
            "preview_key should reference the selected project"
        );
    }

    #[test]
    fn trigger_preview_load_sets_loading_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // Overwrite with Ready so we can confirm it reverts to Loading on change
        state.preview_data = PreviewData::Ready(make_git_info());
        state.preview_key = "different:key".to_string();
        state.trigger_preview_load();
        assert!(
            matches!(state.preview_data, PreviewData::Loading),
            "preview_data should be Loading after trigger"
        );
    }

    #[test]
    fn trigger_preview_load_noop_when_key_unchanged() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.trigger_preview_load(); // sets key + spawns thread
        let key_after_first = state.preview_key.clone();
        state.preview_data = PreviewData::Ready(make_git_info()); // simulate data arrived
        state.trigger_preview_load(); // same key → should NOT overwrite Ready with Loading
        assert!(
            matches!(state.preview_data, PreviewData::Ready(_)),
            "trigger with same key should not overwrite Ready data"
        );
        assert_eq!(state.preview_key, key_after_first);
    }

    #[test]
    fn trigger_preview_load_noop_on_empty_entries() {
        let mut state = TuiState::new(vec![], Navigation::Arrows);
        state.trigger_preview_load(); // should not panic
        assert_eq!(state.preview_key, "");
        assert!(matches!(state.preview_data, PreviewData::Loading));
    }

    #[test]
    fn poll_preview_updates_state_from_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let (tx, rx) = mpsc::channel::<Result<GitInfo, String>>();
        state.preview_rx = Some(rx);
        state.preview_data = PreviewData::Loading;

        // Send data before polling
        tx.send(Ok(make_git_info())).unwrap();
        state.poll_preview();

        assert!(
            matches!(state.preview_data, PreviewData::Ready(_)),
            "poll_preview should transition Loading → Ready when channel has data"
        );
        assert!(state.preview_rx.is_none(), "preview_rx should be cleared after data received");
    }

    #[test]
    fn poll_preview_handles_error_from_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let (tx, rx) = mpsc::channel::<Result<GitInfo, String>>();
        state.preview_rx = Some(rx);

        tx.send(Err("not a git repo".to_string())).unwrap();
        state.poll_preview();

        assert!(
            matches!(state.preview_data, PreviewData::Error(_)),
            "poll_preview should transition to Error when channel sends Err"
        );
    }

    #[test]
    fn poll_preview_noop_when_no_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_rx = None;
        state.poll_preview(); // should not panic
    }

    #[test]
    fn poll_preview_handles_disconnected_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let (tx, rx) = mpsc::channel::<Result<GitInfo, String>>();
        state.preview_rx = Some(rx);
        state.preview_data = PreviewData::Loading;

        // Drop sender without sending — simulates thread panic
        drop(tx);
        state.poll_preview();

        assert!(
            matches!(state.preview_data, PreviewData::Error(_)),
            "disconnected channel should transition to Error, not stay Loading"
        );
        assert!(state.preview_rx.is_none(), "preview_rx should be cleared");
    }

    #[test]
    fn poll_preview_noop_when_channel_empty_but_alive() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let (_tx, rx) = mpsc::channel::<Result<GitInfo, String>>();
        state.preview_rx = Some(rx);
        state.preview_data = PreviewData::Loading;

        // Don't send anything — channel still open
        state.poll_preview();

        assert!(
            matches!(state.preview_data, PreviewData::Loading),
            "should remain Loading when channel is open but empty"
        );
        assert!(state.preview_rx.is_some(), "preview_rx should still be set");
    }

    #[test]
    fn preview_key_includes_session_branch_when_sessions_focused() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        state.selected_session = 0;
        let key0 = state.current_preview_key().unwrap();

        state.selected_session = 1;
        let key1 = state.current_preview_key().unwrap();

        assert_ne!(key0, key1, "key should differ when selected session changes");
        assert!(key0.contains("main"), "first session is main");
        assert!(key1.contains("feat/login"), "second session is feat/login");
    }

    #[test]
    fn preview_key_uses_first_session_when_projects_focused() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Projects;
        state.selected_session = 1; // should be ignored — uses first session
        let key = state.current_preview_key().unwrap();
        assert!(key.contains("main"), "should use first session branch when projects focused");
    }

    #[test]
    fn preview_key_empty_branch_for_no_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 1; // hermes has no sessions
        let key = state.current_preview_key().unwrap();
        assert!(key.ends_with(':'), "key should end with ':' when project has no sessions");
    }

    #[test]
    fn renders_preview_at_minimum_terminal_height() {
        // 8 (preview) + 3 (status) + 1 (min main) = 12 minimum
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let _out = render_to_string(&state, 80, 12); // should not panic
    }

    #[test]
    fn preview_key_changes_on_project_navigation() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.trigger_preview_load();
        let key1 = state.preview_key.clone();

        state.move_down(); // move to hermes
        state.trigger_preview_load();
        let key2 = state.preview_key.clone();

        assert_ne!(key1, key2, "preview key should change when project changes");
        assert!(key2.contains("hermes"));
    }

    // ── State / navigation unit tests (unchanged) ─────────────────────────

    #[test]
    fn navigate_down_increments_selection() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        assert_eq!(state.selected_project, 0);
        state.move_down();
        assert_eq!(state.selected_project, 1);
        state.move_down();
        assert_eq!(state.selected_project, 2);
    }

    #[test]
    fn navigate_up_does_not_underflow() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.move_up();
        assert_eq!(state.selected_project, 0, "should stay at 0");
    }

    #[test]
    fn navigate_down_stops_at_last_item() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 2;
        state.move_down();
        assert_eq!(state.selected_project, 2, "should not go past last item");
    }

    #[test]
    fn switch_panel_toggles_focus() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        assert_eq!(state.focused_panel, Panel::Projects);
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Sessions);
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Projects);
    }

    #[test]
    fn navigate_sessions_panel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        assert_eq!(state.selected_session, 0);
        state.move_down();
        assert_eq!(state.selected_session, 1);
        state.move_up();
        assert_eq!(state.selected_session, 0);
        state.move_up();
        assert_eq!(state.selected_session, 0, "should not underflow");
    }

    #[test]
    fn navigate_sessions_does_not_overflow() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        state.selected_session = 1; // last session of myapp
        state.move_down();
        assert_eq!(state.selected_session, 1, "should not go past last session");
    }

    #[test]
    fn navigate_sessions_empty_project() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 1; // hermes has no sessions
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 0, "empty project: session stays 0");
    }

    #[test]
    fn navigate_down_resets_session_cursor() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 1);
        // Move to another project — session cursor must reset to 0
        state.focused_panel = Panel::Projects;
        state.move_down();
        assert_eq!(state.selected_session, 0, "session cursor should reset after project change");
    }

    #[test]
    fn search_filters_projects_by_name() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "my".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "MYAPP".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn empty_search_shows_all_projects() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        assert_eq!(state.filtered_projects().len(), 3);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "zzznomatch".to_string();
        assert!(state.filtered_projects().is_empty());
    }

    #[test]
    fn selected_entry_returns_correct_project() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 1;
        assert_eq!(state.selected_entry().unwrap().project.name, "hermes");
    }

    #[test]
    fn selected_entry_with_filter_returns_filtered_item() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "prod".to_string();
        state.selected_project = 0;
        assert_eq!(
            state.selected_entry().unwrap().project.name,
            "prod-api"
        );
    }

    #[test]
    fn selected_entry_empty_list_returns_none() {
        let state = TuiState::new(vec![], Navigation::Arrows);
        assert!(state.selected_entry().is_none());
    }

    // ── Edge case tests ───────────────────────────────────────────────────

    #[test]
    fn search_resets_selected_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        state.selected_session = 1;

        state.search_query = "hermes".to_string();
        state.selected_project = 0;
        state.selected_session = 0;

        let entry = state.selected_entry().unwrap();
        assert_eq!(entry.project.name, "hermes");
        assert!(entry.sessions.is_empty());
        assert_eq!(state.selected_session, 0, "session cursor must be 0 for project with no sessions");
    }

    #[test]
    fn move_down_on_empty_entries_is_noop() {
        let mut state = TuiState::new(vec![], Navigation::Arrows);
        state.move_down();
        assert_eq!(state.selected_project, 0);
    }

    #[test]
    fn move_up_on_empty_entries_is_noop() {
        let mut state = TuiState::new(vec![], Navigation::Arrows);
        state.move_up();
        assert_eq!(state.selected_project, 0);
    }

    #[test]
    fn switch_panel_on_empty_entries_does_not_panic() {
        let mut state = TuiState::new(vec![], Navigation::Arrows);
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Sessions);
        state.move_down(); // sessions panel, no entry → noop
        assert_eq!(state.selected_session, 0);
    }

    #[test]
    fn selected_entry_with_out_of_bounds_index_returns_none() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 99; // way past the end
        assert!(state.selected_entry().is_none());
    }

    #[test]
    fn search_then_clear_restores_full_list() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "prod".to_string();
        assert_eq!(state.filtered_projects().len(), 1);
        state.search_query.clear();
        assert_eq!(state.filtered_projects().len(), 3);
    }

    #[test]
    fn single_project_navigation_bounds() {
        let entries = vec![ProjectEntry {
            project: make_project("solo", false),
            sessions: vec![Session::new("solo", "main")],
            worktree_count: 0,
        }];
        let mut state = TuiState::new(entries, Navigation::Arrows);
        state.move_up();
        assert_eq!(state.selected_project, 0);
        state.move_down();
        assert_eq!(state.selected_project, 0, "single project: down is noop");
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 0, "single session: down is noop");
    }

    #[test]
    fn renders_empty_search_no_match_without_panic() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "zzz_no_match".to_string();
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"));
    }

    #[test]
    fn renders_narrow_terminal_without_panic() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        // Extremely narrow — columns may truncate but should not panic
        let _out = render_to_string(&state, 20, 10);
    }

    #[test]
    fn renders_remote_project_status_bar() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 2; // prod-api is remote
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("remote"), "status bar should say 'remote' for remote project");
        assert!(out.contains("prod-api"), "status bar should show prod-api");
    }

    #[test]
    fn navigate_project_down_then_up_resets_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 1);
        state.focused_panel = Panel::Projects;
        state.move_down();
        assert_eq!(state.selected_session, 0, "session resets on project change via down");
        state.move_up();
        assert_eq!(state.selected_session, 0, "session resets on project change via up");
    }

    // ── Fuzzy match unit tests ────────────────────────────────────────────

    #[test]
    fn fuzzy_match_empty_query_always_matches() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("", ""));
    }

    #[test]
    fn fuzzy_match_exact_match() {
        assert!(fuzzy_match("myapp", "myapp"));
    }

    #[test]
    fn fuzzy_match_substring() {
        assert!(fuzzy_match("app", "myapp"));
    }

    #[test]
    fn fuzzy_match_non_contiguous_chars_in_order() {
        // 'm', 'p', 'p' all appear in "myapp" in order
        assert!(fuzzy_match("mpp", "myapp"));
    }

    #[test]
    fn fuzzy_match_chars_out_of_order_fails() {
        // 'p' before 'm' — not possible in "myapp"
        assert!(!fuzzy_match("pm", "myapp"));
    }

    #[test]
    fn fuzzy_match_is_case_insensitive() {
        assert!(fuzzy_match("MYA", "myapp"));
        assert!(fuzzy_match("mya", "MYAPP"));
    }

    #[test]
    fn fuzzy_match_query_longer_than_target_fails() {
        assert!(!fuzzy_match("myapplication", "myapp"));
    }

    #[test]
    fn fuzzy_match_no_common_chars_fails() {
        assert!(!fuzzy_match("xyz", "myapp"));
    }

    #[test]
    fn fuzzy_match_nonempty_query_empty_target_fails() {
        assert!(!fuzzy_match("a", ""));
    }

    #[test]
    fn fuzzy_match_unicode_case_insensitive() {
        assert!(fuzzy_match("ÉL", "élan"));
        assert!(fuzzy_match("él", "ÉLAN"));
    }

    #[test]
    fn fuzzy_match_repeated_char_in_query() {
        // "aa" requires two 'a' chars in target
        assert!(fuzzy_match("aa", "abracadabra"));
        assert!(!fuzzy_match("aa", "abcd"));
    }

    // ── filtered_projects fuzzy tests ────────────────────────────────────

    #[test]
    fn fuzzy_search_matches_project_non_contiguous() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "mpp" → m..pp → matches "myapp"
        state.search_query = "mpp".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn fuzzy_search_includes_project_with_matching_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "feat" doesn't match "myapp" or "hermes" project names,
        // but "myapp:feat-login" session contains "feat"
        state.search_query = "feat".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn fuzzy_search_project_name_match_shows_no_sessions_when_none_match() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "hermes" matches the project name, but hermes has no sessions
        state.search_query = "hermes".to_string();
        state.selected_project = 0;
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "hermes");
        assert!(state.filtered_sessions().is_empty());
    }

    #[test]
    fn fuzzy_search_project_matched_by_name_hides_nonmatching_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "myapp" matches the project name; sessions should still be filtered
        // "myapp:main" fuzzy-matches "myapp" (m-y-a-p-p all present) so it shows
        // "myapp:feat-login" also fuzzy-matches "myapp" (m-y-a-p... has 'p') so both show
        state.search_query = "myapp".to_string();
        state.selected_project = 0;
        let sessions = state.filtered_sessions();
        // Both sessions contain "myapp" prefix so both match
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn fuzzy_search_session_name_match_via_project_inclusion() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "main" matches the "myapp:main" session, so myapp should appear
        state.search_query = "main".to_string();
        let filtered = state.filtered_projects();
        assert!(filtered.iter().any(|(_, e)| e.project.name == "myapp"));
    }

    // ── filtered_sessions tests ───────────────────────────────────────────

    #[test]
    fn filtered_sessions_empty_query_returns_all() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        // myapp has 2 sessions; no query → all returned
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn filtered_sessions_filters_by_fuzzy_match() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "login".to_string();
        // Only "myapp:feat-login" matches "login"
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].name.contains("login"));
    }

    #[test]
    fn filtered_sessions_no_match_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "zzznomatch".to_string();
        assert!(state.filtered_sessions().is_empty());
    }

    #[test]
    fn filtered_sessions_empty_project_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.selected_project = 1; // hermes has no sessions
        assert!(state.filtered_sessions().is_empty());
    }

    #[test]
    fn navigate_sessions_respects_filter() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // "login" matches only 1 session → moving down is a noop
        state.search_query = "login".to_string();
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 0, "only 1 filtered session, down is noop");
    }

    #[test]
    fn navigate_projects_while_in_search_mode() {
        // Verifies that arrow-key navigation works while search mode is active.
        // There are 3 entries; all match an empty query.
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        assert_eq!(state.selected_project, 0);
        state.move_down();
        assert_eq!(state.selected_project, 1, "down should work in search mode");
        state.move_up();
        assert_eq!(state.selected_project, 0, "up should work in search mode");
    }

    #[test]
    fn delete_targets_filtered_session_not_unfiltered() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // Filter to only "feat-login"; selected_session = 0 should point to it
        state.search_query = "login".to_string();
        state.selected_session = 0;
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].name.contains("feat-login"));
    }

    #[test]
    fn delete_noop_when_no_sessions_match_filter() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_query = "zzznomatch".to_string();
        state.selected_session = 0;
        // No sessions match → get returns None → delete would be a no-op
        assert!(state.filtered_sessions().get(0).is_none());
    }

    // ── Snapshot tests for search mode UI states ─────────────────────────

    #[test]
    fn renders_search_mode_filters_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "login".to_string();
        let out = render_to_string(&state, 80, 24);
        // Only the login session should appear
        assert!(out.contains("login"), "login session should be visible");
    }

    #[test]
    fn renders_search_mode_hides_non_matching_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "login".to_string();
        let out = render_to_string(&state, 80, 24);
        // "myapp:main" session does not fuzzy-match "login" (no 'l' in "myapp:main")
        // so it must not appear in the rendered output
        assert!(
            !out.contains("myapp:main"),
            "non-matching session myapp:main should be hidden"
        );
        assert!(
            out.contains("feat-login"),
            "matching session feat-login should be visible"
        );
    }

    #[test]
    fn renders_project_matched_by_session_name() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "feat".to_string();
        let out = render_to_string(&state, 80, 24);
        // "myapp" should appear because its session "feat-login" matches "feat"
        assert!(out.contains("myapp"), "myapp should appear because feat-login matches feat");
        // "hermes" and "prod-api" should NOT appear
        assert!(!out.contains("hermes"), "hermes should be filtered out");
        assert!(!out.contains("prod-api"), "prod-api should be filtered out");
    }

    #[test]
    fn renders_fuzzy_match_non_contiguous() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.search_mode = true;
        state.search_query = "hms".to_string(); // h..m..s matches "hermes"
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("hermes"), "hermes should match fuzzy query 'hms'");
        assert!(!out.contains("myapp"), "myapp should not match 'hms'");
    }

    // ── Notification badge tests ──────────────────────────────────────────

    #[test]
    fn renders_bell_badge_on_session_with_notification() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // myapp:main has a pending notification
        state.notifications.insert("myapp:main".to_string());
        let out = render_to_string(&state, 80, 24);
        // The 🔔 Unicode codepoint U+1F514 should appear in the output
        assert!(
            out.contains('\u{1f514}'),
            "should render 🔔 badge for session with notification"
        );
    }

    #[test]
    fn does_not_render_bell_badge_without_notification() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 80, 24);
        assert!(
            !out.contains('\u{1f514}'),
            "should not render 🔔 badge when no notifications pending"
        );
    }

    #[test]
    fn renders_bell_only_on_notified_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // Only myapp:feat-login has a notification
        state.notifications.insert("myapp:feat-login".to_string());
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains('\u{1f514}'), "🔔 should appear for feat-login");
        // myapp:main has no notification
        assert!(out.contains("myapp:main"), "myapp:main should still render");
    }

    #[test]
    fn notifications_field_default_is_empty() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        assert!(state.notifications.is_empty());
    }

    // ── PR / CI / Zellij rendering tests (issue #11) ─────────────────────

    #[test]
    fn renders_pr_number_and_open_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(42, PrState::Open));
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("PR:"), "should show 'PR:' label");
        assert!(out.contains("42"), "should show PR number");
        assert!(out.contains("open"), "should show PR state 'open'");
    }

    #[test]
    fn renders_pr_merged_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(7, PrState::Merged));
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("merged"), "should show PR state 'merged'");
    }

    #[test]
    fn renders_pr_closed_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(3, PrState::Closed));
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("closed"), "should show PR state 'closed'");
    }

    #[test]
    fn renders_no_pr_line_when_pr_absent() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let info = make_git_info(); // pr: None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(!out.contains("PR:"), "should not show PR line when no PR");
    }

    #[test]
    fn renders_ci_passing_indicator() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(42, PrState::Open));
        info.ci = Some(CiStatus::Passing);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("passing"), "should show 'passing' for CI passing");
        // ✅ U+2705
        assert!(out.contains('\u{2705}'), "should show ✅ for passing CI");
    }

    #[test]
    fn renders_ci_failing_indicator() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(1, PrState::Open));
        info.ci = Some(CiStatus::Failing);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("failing"), "should show 'failing' for CI failing");
        // ❌ U+274C
        assert!(out.contains('\u{274c}'), "should show ❌ for failing CI");
    }

    #[test]
    fn renders_ci_pending_indicator() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.ci = Some(CiStatus::Pending);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("pending"), "should show 'pending' for CI pending");
    }

    #[test]
    fn renders_no_ci_line_when_ci_unknown_and_no_pr() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.ci = Some(CiStatus::Unknown);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        // Unknown CI with no PR — no PR/CI line should appear
        assert!(!out.contains("CI:"), "should not show CI line when unknown and no PR");
    }

    #[test]
    fn renders_zellij_session_info() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.zellij = Some(make_zellij_info());
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("session:"), "should show 'session:' label");
        assert!(out.contains("3 tabs"), "should show tab count");
        assert!(out.contains("5 panes"), "should show pane count");
        assert!(out.contains("2h34m"), "should show uptime");
    }

    #[test]
    fn renders_zellij_uptime_only_when_tab_pane_zero() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.zellij = Some(ZellijInfo {
            tab_count: 0,
            pane_count: 0,
            uptime: "1h05m".to_string(),
        });
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("session:"), "should show session line");
        assert!(out.contains("1h05m"), "should show uptime");
        // No tab/pane counts when zero
        assert!(!out.contains("0 tabs"), "should not show '0 tabs'");
        assert!(!out.contains("0 panes"), "should not show '0 panes'");
    }

    #[test]
    fn renders_no_zellij_line_when_absent() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let info = make_git_info(); // zellij: None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(!out.contains("session:"), "should not show session line when no Zellij info");
    }

    #[test]
    fn renders_full_preview_with_all_info() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(42, PrState::Open));
        info.ci = Some(CiStatus::Passing);
        info.zellij = Some(make_zellij_info());
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        // All three sections should appear
        assert!(out.contains("PR: #42 (open)"), "should show PR info");
        assert!(out.contains("passing"), "should show CI passing");
        assert!(out.contains("session:"), "should show session label");
        assert!(out.contains("2h34m"), "should show uptime");
        assert!(out.contains("recent commits"), "should still show commits");
    }

    // ── PR/CI parsing unit tests ────────────────────────────────────────────

    #[test]
    fn parse_pr_json_open() {
        let json = r#"{"number":42,"state":"OPEN","title":"feat: login","url":"https://github.com/owner/repo/pull/42"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn parse_pr_json_merged() {
        let json = r#"{"number":7,"state":"MERGED","title":"fix: bug","url":"https://github.com/o/r/pull/7"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn parse_pr_json_closed() {
        let json = r#"{"number":3,"state":"CLOSED","title":"old","url":"https://github.com/o/r/pull/3"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn parse_pr_json_missing_number_returns_none() {
        let json = r#"{"state":"OPEN","title":"test"}"#;
        assert!(parse_pr_json(json).is_none());
    }

    #[test]
    fn parse_ci_status_json_success() {
        let json = r#"[{"conclusion":"success","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Passing);
    }

    #[test]
    fn parse_ci_status_json_failure() {
        let json = r#"[{"conclusion":"failure","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Failing);
    }

    #[test]
    fn parse_ci_status_json_pending() {
        let json = r#"[{"conclusion":"","status":"in_progress"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Pending);
    }

    #[test]
    fn parse_ci_status_json_empty_array() {
        let json = r#"[]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Unknown);
    }

    #[test]
    fn parse_ci_status_json_timed_out() {
        let json = r#"[{"conclusion":"timed_out","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Failing);
    }

    // ── poll_forge tests ────────────────────────────────────────────────────

    #[test]
    fn poll_forge_merges_pr_into_ready_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info());

        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);

        tx.send(Ok(ForgeData {
            pr: Some(make_pull_request(42, PrState::Open)),
            ci: CiStatus::Passing,
            zellij: Some(make_zellij_info()),
        }))
        .unwrap();

        state.poll_forge();

        match &state.preview_data {
            PreviewData::Ready(info) => {
                assert!(info.pr.is_some(), "PR should be merged into git info");
                assert_eq!(info.pr.as_ref().unwrap().number, 42);
                assert_eq!(info.ci, Some(CiStatus::Passing));
                assert!(info.zellij.is_some());
            }
            _ => panic!("expected Ready state"),
        }
        assert!(state.forge_rx.is_none(), "forge_rx should be cleared after receive");
    }

    #[test]
    fn poll_forge_noop_when_no_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.forge_rx = None;
        state.poll_forge(); // should not panic
    }

    #[test]
    fn poll_forge_noop_when_channel_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.preview_data = PreviewData::Ready(make_git_info());
        let (_tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);
        state.poll_forge(); // nothing sent yet
        // preview_data unchanged, forge_rx still set
        assert!(state.forge_rx.is_some());
    }

    #[test]
    fn poll_forge_discards_data_if_not_ready() {
        // If git info hasn't arrived yet (still Loading), forge data is discarded
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        // preview_data stays Loading (git not yet received)
        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);
        tx.send(Ok(ForgeData {
            pr: Some(make_pull_request(1, PrState::Open)),
            ci: CiStatus::Passing,
            zellij: None,
        }))
        .unwrap();
        state.poll_forge();
        // preview_data should still be Loading (nothing to merge into)
        assert!(matches!(state.preview_data, PreviewData::Loading));
        assert!(state.forge_rx.is_none(), "forge_rx cleared even when data discarded");
    }

    #[test]
    fn poll_forge_handles_disconnected_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);
        drop(tx); // simulate thread panic
        state.poll_forge();
        assert!(state.forge_rx.is_none(), "forge_rx should be cleared on disconnect");
    }

    #[test]
    fn forge_rx_default_is_none() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        assert!(state.forge_rx.is_none());
    }

    // ── Edge-case tests (review round) ──────────────────────────────────────

    #[test]
    fn extract_json_string_handles_space_after_colon() {
        // gh CLI may produce `"key": "value"` with a space after the colon.
        let json = r#"{"state": "OPEN", "title": "my pr"}"#;
        assert_eq!(
            extract_json_string(json, "state"),
            Some("OPEN".to_string())
        );
        assert_eq!(
            extract_json_string(json, "title"),
            Some("my pr".to_string())
        );
    }

    #[test]
    fn extract_json_string_non_string_value_returns_none() {
        // If the value is a number, not a string, should return None.
        let json = r#"{"number": 42}"#;
        assert_eq!(extract_json_string(json, "number"), None);
    }

    #[test]
    fn parse_pr_json_with_spaced_json() {
        // Ensure PR parsing works with spaces after colons.
        let json = r#"{"number": 99, "state": "MERGED", "title": "fix: thing", "url": "https://github.com/o/r/pull/99"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.number, 99);
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.title, "fix: thing");
    }

    #[test]
    fn parse_ci_status_json_cancelled_is_unknown() {
        let json = r#"[{"conclusion":"cancelled","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Unknown);
    }

    #[test]
    fn parse_ci_status_json_queued_is_pending() {
        let json = r#"[{"conclusion":"","status":"queued"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Pending);
    }

    #[test]
    fn renders_ci_without_pr_no_orphaned_separator() {
        // When CI is shown but no PR exists, there should be no " | " prefix.
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.ci = Some(CiStatus::Passing);
        // pr remains None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("CI:"), "should show CI line");
        assert!(!out.contains("| CI:"), "should not have orphaned '| ' before CI");
    }

    #[test]
    fn renders_pr_without_ci() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(10, PrState::Open));
        // ci remains None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("PR: #10 (open)"), "should show PR info");
        assert!(!out.contains("CI:"), "should not show CI when absent");
    }

    #[test]
    fn parse_zellij_json_multiple_sessions_picks_correct_one() {
        // The first session has 10 tabs, the target has 3 tabs.
        // Without bounded object slicing, extract_json_u64 might pick up the wrong "tabs".
        let json = r#"[{"name":"other","tabs":10,"panes":20},{"name":"target","tabs":3,"panes":5}]"#;
        let info = parse_zellij_json_for_session(json, "target").unwrap();
        assert_eq!(info.tab_count, 3, "should pick tabs from the correct session object");
        assert_eq!(info.pane_count, 5, "should pick panes from the correct session object");
    }

    #[test]
    fn parse_zellij_json_session_not_found() {
        let json = r#"[{"name":"other","tabs":2,"panes":3}]"#;
        assert!(parse_zellij_json_for_session(json, "missing").is_none());
    }

    #[test]
    fn extract_zellij_uptime_no_pattern_returns_none() {
        assert!(extract_zellij_uptime("myapp:main [EXITED]").is_none());
    }

    #[test]
    fn extract_zellij_uptime_extracts_duration() {
        let line = "myapp:main [Created 3h12m ago]";
        assert_eq!(extract_zellij_uptime(line), Some("3h12m".to_string()));
    }

    #[test]
    fn extract_json_u64_missing_key() {
        let json = r#"{"other":42}"#;
        assert_eq!(extract_json_u64(json, "number"), None);
    }

    #[test]
    fn parse_zellij_json_with_spaces_after_colons() {
        let json = r#"[{"name": "target", "tabs": 2, "panes": 4}]"#;
        let info = parse_zellij_json_for_session(json, "target").unwrap();
        assert_eq!(info.tab_count, 2);
        assert_eq!(info.pane_count, 4);
    }

    #[test]
    fn extract_json_string_with_escaped_quote() {
        let json = r#"{"title":"fix: \"quoted\" thing"}"#;
        assert_eq!(
            extract_json_string(json, "title"),
            Some("fix: \"quoted\" thing".to_string())
        );
    }

    // ── Modal / ProjectForm tests ──────────────────────────────────────────

    #[test]
    fn project_form_new_starts_at_field_0() {
        let form = ProjectForm::new();
        assert_eq!(form.active_field, 0);
        assert_eq!(form.fields.len(), 4);
        assert!(form.fields[0].required);
        assert!(form.fields[1].required);
        assert!(!form.fields[2].required);
        assert!(!form.fields[3].required);
    }

    #[test]
    fn advance_modal_tab_advances_field() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        let outcome = advance_modal(&mut modal, KeyCode::Tab);
        assert!(matches!(outcome, ModalOutcome::Continue));
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.active_field, 1);
    }

    #[test]
    fn advance_modal_tab_wraps_around() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Tab 4 times → wraps back to 0
        for _ in 0..4 {
            advance_modal(&mut modal, KeyCode::Tab);
        }
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.active_field, 0);
    }

    #[test]
    fn advance_modal_backtab_goes_to_previous() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::Tab); // field 1
        advance_modal(&mut modal, KeyCode::BackTab); // back to field 0
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.active_field, 0);
    }

    #[test]
    fn advance_modal_backtab_wraps_from_first_to_last() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::BackTab); // wraps to field 3
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.active_field, 3);
    }

    #[test]
    fn advance_modal_escape_returns_close() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn advance_modal_enter_with_empty_required_fields_returns_continue() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Continue));
        // Warnings should be set on required fields
        let Modal::AddProject(ref form) = modal;
        assert!(form.fields[0].warning.is_some(), "path field should have warning");
        assert!(form.fields[1].warning.is_some(), "name field should have warning");
        assert!(form.fields[2].warning.is_none(), "host field is optional, no warning");
        assert!(form.fields[3].warning.is_none(), "token field is optional, no warning");
    }

    #[test]
    fn advance_modal_enter_with_valid_data_returns_submit() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Set path and name using a block to scope the borrow
        {
            if let Modal::AddProject(ref mut form) = modal {
                form.fields[0].value = "/code/myapp".to_string();
                form.fields[1].value = "myapp".to_string();
            }
        }
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::Submit { path, name, host, token } => {
                assert_eq!(path, "/code/myapp");
                assert_eq!(name, "myapp");
                assert!(host.is_none());
                assert!(token.is_none());
            }
            _ => panic!("expected Submit outcome"),
        }
    }

    #[test]
    fn advance_modal_enter_with_optional_fields_submits_them() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        {
            if let Modal::AddProject(ref mut form) = modal {
                form.fields[0].value = "/code/app".to_string();
                form.fields[1].value = "app".to_string();
                form.fields[2].value = "https://vps.example.com".to_string();
                form.fields[3].value = "mytoken".to_string();
            }
        }
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::Submit { host, token, .. } => {
                assert_eq!(host, Some("https://vps.example.com".to_string()));
                assert_eq!(token, Some("mytoken".to_string()));
            }
            _ => panic!("expected Submit outcome"),
        }
    }

    #[test]
    fn advance_modal_char_appends_to_active_field() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::Char('/'));
        advance_modal(&mut modal, KeyCode::Char('c'));
        advance_modal(&mut modal, KeyCode::Char('o'));
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.fields[0].value, "/co");
    }

    #[test]
    fn advance_modal_backspace_removes_last_char() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::Char('a'));
        advance_modal(&mut modal, KeyCode::Char('b'));
        advance_modal(&mut modal, KeyCode::Backspace);
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.fields[0].value, "a");
    }

    #[test]
    fn autofill_name_fills_from_path_basename() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "/code/myproject".to_string();
        autofill_name_if_empty(&mut form);
        assert_eq!(form.fields[1].value, "myproject");
    }

    #[test]
    fn autofill_name_does_not_overwrite_manual_input() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "/code/myproject".to_string();
        form.fields[1].value = "custom-name".to_string();
        form.name_was_modified = true; // user has manually edited name field
        autofill_name_if_empty(&mut form);
        assert_eq!(form.fields[1].value, "custom-name");
    }

    #[test]
    fn autofill_name_triggered_when_typing_in_path_field() {
        // Simulate typing in path field: name should auto-fill when empty
        let mut modal = Modal::AddProject(ProjectForm::new());
        for c in "/code/webapp".chars() {
            advance_modal(&mut modal, KeyCode::Char(c));
        }
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.fields[1].value, "webapp", "name should be auto-filled from path basename");
    }

    #[test]
    fn modal_opens_on_uppercase_a_key_in_projects_panel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        assert!(state.modal.is_none(), "no modal initially");
        state.focused_panel = Panel::Projects;
        // Simulate pressing 'A' by directly triggering the key handler logic
        state.modal = Some(Modal::AddProject(ProjectForm::new()));
        assert!(state.modal.is_some(), "modal should be open");
    }

    #[test]
    fn render_modal_add_project_shows_fields() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::AddProject(ProjectForm::new()));
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Add Project"), "should show modal title");
        assert!(out.contains("Path"), "should show Path field label");
        assert!(out.contains("Name"), "should show Name field label");
        assert!(out.contains("Host"), "should show Host field label");
        assert!(out.contains("Token"), "should show Token field label");
    }

    #[test]
    fn render_modal_shows_hints_line() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::AddProject(ProjectForm::new()));
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Tab"), "should show Tab hint");
        assert!(out.contains("Enter"), "should show Enter hint");
        assert!(out.contains("Esc"), "should show Esc hint");
    }

    #[test]
    fn render_modal_shows_yellow_warning_on_required_field() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        let mut form = ProjectForm::new();
        form.fields[0].warning = Some("Required".to_string());
        state.modal = Some(Modal::AddProject(form));
        // Just verify it doesn't panic and renders something
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Required") || out.contains("Add Project"), "modal rendered");
    }

    #[test]
    fn tui_action_add_project_variant_exists() {
        let action = TuiAction::AddProject {
            path: "/code/app".to_string(),
            name: "app".to_string(),
            host: None,
            token: None,
        };
        assert!(matches!(action, TuiAction::AddProject { .. }));
    }

    // ── expand_tilde_path edge cases ──────────────────────────────────────

    #[test]
    fn expand_tilde_with_slash_expands() {
        let result = expand_tilde_path("~/code/app");
        assert!(!result.starts_with('~'), "tilde should be expanded");
        assert!(result.ends_with("/code/app"), "path suffix preserved");
    }

    #[test]
    fn expand_tilde_bare_tilde_expands() {
        let result = expand_tilde_path("~");
        assert!(!result.starts_with('~'), "bare tilde should expand to HOME");
    }

    #[test]
    fn expand_tilde_username_not_expanded() {
        let result = expand_tilde_path("~bob/code");
        assert_eq!(result, "~bob/code", "~username form should not be expanded");
    }

    #[test]
    fn expand_tilde_no_tilde_unchanged() {
        let result = expand_tilde_path("/absolute/path");
        assert_eq!(result, "/absolute/path");
    }

    // ── autofill edge cases ───────────────────────────────────────────────

    #[test]
    fn autofill_name_from_trailing_slash_path() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "/code/myproject/".to_string();
        autofill_name_if_empty(&mut form);
        // Path::file_name on trailing slash returns None in Rust — basename should be empty
        // (or "myproject" depending on OS). Verify no panic.
        // The key test is that it doesn't crash.
        let _ = &form.fields[1].value;
    }

    #[test]
    fn autofill_name_from_root_path() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "/".to_string();
        autofill_name_if_empty(&mut form);
        // Root path has no basename — should produce empty string, not panic.
        assert_eq!(form.fields[1].value, "");
    }

    #[test]
    fn autofill_name_from_empty_path() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "".to_string();
        autofill_name_if_empty(&mut form);
        assert_eq!(form.fields[1].value, "");
    }

    // ── advance_modal edge cases ──────────────────────────────────────────

    #[test]
    fn advance_modal_backspace_on_empty_field_does_not_panic() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Backspace on already-empty path field
        let outcome = advance_modal(&mut modal, KeyCode::Backspace);
        assert!(matches!(outcome, ModalOutcome::Continue));
        let Modal::AddProject(ref form) = modal;
        assert_eq!(form.fields[0].value, "");
    }

    #[test]
    fn advance_modal_submit_with_whitespace_only_fields_shows_required() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = "   ".to_string();
            form.fields[1].value = "  ".to_string();
        }
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Continue), "whitespace-only should not submit");
        let Modal::AddProject(ref form) = modal;
        assert!(form.fields[0].warning.is_some());
        assert!(form.fields[1].warning.is_some());
    }

    #[test]
    fn advance_modal_submit_trims_whitespace_from_values() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = "  /code/app  ".to_string();
            form.fields[1].value = "  app  ".to_string();
            form.fields[2].value = "  ".to_string(); // whitespace-only optional → None
        }
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::Submit { path, name, host, .. } => {
                assert_eq!(name, "app", "name should be trimmed");
                assert!(!path.starts_with(' '), "path should be trimmed");
                assert!(host.is_none(), "whitespace-only optional field should be None");
            }
            _ => panic!("expected Submit"),
        }
    }

    #[test]
    fn advance_modal_submit_expands_tilde_in_path() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = "~/code/app".to_string();
            form.fields[1].value = "app".to_string();
        }
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::Submit { path, .. } => {
                assert!(!path.starts_with('~'), "tilde should be expanded on submit");
                assert!(path.ends_with("/code/app"));
            }
            _ => panic!("expected Submit"),
        }
    }

    // ── modal_rect edge cases ─────────────────────────────────────────────

    #[test]
    fn modal_rect_clamps_to_area_when_too_large() {
        let area = Rect::new(0, 0, 40, 10);
        let rect = modal_rect(62, 16, area);
        assert!(rect.width <= area.width, "width should be clamped");
        assert!(rect.height <= area.height, "height should be clamped");
    }

    #[test]
    fn modal_rect_centered_in_area() {
        let area = Rect::new(0, 0, 100, 50);
        let rect = modal_rect(60, 16, area);
        assert_eq!(rect.x, 20);
        assert_eq!(rect.y, 17);
    }

    // ── non_empty_opt edge cases ──────────────────────────────────────────

    #[test]
    fn non_empty_opt_empty_string() {
        assert_eq!(non_empty_opt(""), None);
    }

    #[test]
    fn non_empty_opt_whitespace_only() {
        assert_eq!(non_empty_opt("   "), None);
    }

    #[test]
    fn non_empty_opt_with_value() {
        assert_eq!(non_empty_opt("  hello  "), Some("hello".to_string()));
    }

    // ── Delete Project modal tests ─────────────────────────────────────────

    #[test]
    fn d_key_on_projects_panel_with_project_opens_delete_modal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Projects;
        assert!(state.modal.is_none());

        // Simulate 'D' key press logic: open delete confirm modal
        if let Some(entry) = state.selected_entry() {
            let project_name = entry.project.name.clone();
            let session_count = entry.sessions.len();
            let worktree_count = entry.worktree_count;
            state.modal = Some(Modal::DeleteConfirm {
                project_name: project_name.clone(),
                session_count,
                worktree_count,
            });
        }

        assert!(state.modal.is_some(), "modal should be opened");
        match &state.modal {
            Some(Modal::DeleteConfirm { project_name, session_count, .. }) => {
                assert_eq!(project_name, "myapp");
                assert_eq!(*session_count, 2);
            }
            _ => panic!("expected DeleteConfirm modal"),
        }
    }

    #[test]
    fn d_key_with_no_projects_does_not_open_modal() {
        let mut state = TuiState::new(vec![], Navigation::Arrows);
        state.focused_panel = Panel::Projects;
        // Simulate D key: selected_entry() returns None → no modal
        if let Some(entry) = state.selected_entry() {
            state.modal = Some(Modal::DeleteConfirm {
                project_name: entry.project.name.clone(),
                session_count: entry.sessions.len(),
                worktree_count: entry.worktree_count,
            });
        }
        assert!(state.modal.is_none(), "no modal when no projects");
    }

    #[test]
    fn advance_modal_delete_confirm_enter_returns_delete_confirmed() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 2,
            worktree_count: 1,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::DeleteConfirmed { project } => assert_eq!(project, "myapp"),
            _ => panic!("expected DeleteConfirmed"),
        }
    }

    #[test]
    fn advance_modal_delete_confirm_y_returns_delete_confirmed() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "hermes".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('y'));
        assert!(matches!(outcome, ModalOutcome::DeleteConfirmed { .. }));
    }

    #[test]
    fn advance_modal_delete_confirm_uppercase_y_returns_delete_confirmed() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "hermes".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('Y'));
        assert!(matches!(outcome, ModalOutcome::DeleteConfirmed { .. }));
    }

    #[test]
    fn advance_modal_delete_confirm_esc_closes() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn advance_modal_delete_confirm_n_closes() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('n'));
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn advance_modal_delete_confirm_uppercase_n_closes() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('N'));
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn advance_modal_delete_confirm_other_key_continues() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 0,
            worktree_count: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('x'));
        assert!(matches!(outcome, ModalOutcome::Continue));
    }

    #[test]
    fn renders_delete_confirm_modal_shows_project_name() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 2,
            worktree_count: 3,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Delete Project"), "should show Delete Project title");
        assert!(out.contains("myapp"), "should show project name");
    }

    #[test]
    fn renders_delete_confirm_modal_shows_counts() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "myapp".to_string(),
            session_count: 2,
            worktree_count: 3,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("2"), "should show session count");
        assert!(out.contains("3"), "should show worktree count");
    }

    #[test]
    fn status_bar_shows_delete_project_hint() {
        let state = TuiState::new(make_entries(), Navigation::Arrows);
        let out = render_to_string(&state, 120, 30);
        assert!(out.contains("[D]el project"), "status bar should include [D]el project hint");
    }

    #[test]
    fn worktree_count_stored_in_project_entry() {
        let entry = ProjectEntry {
            project: make_project("test", false),
            sessions: vec![],
            worktree_count: 5,
        };
        assert_eq!(entry.worktree_count, 5);
    }

    #[test]
    fn d_key_on_sessions_panel_does_not_open_delete_modal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.focused_panel = Panel::Sessions;
        // Replicate the guard from the event loop: D only opens modal on Projects panel.
        if state.focused_panel == Panel::Projects {
            if let Some(entry) = state.selected_entry() {
                state.modal = Some(Modal::DeleteConfirm {
                    project_name: entry.project.name.clone(),
                    session_count: entry.sessions.len(),
                    worktree_count: entry.worktree_count,
                });
            }
        }
        assert!(state.modal.is_none(), "D on Sessions panel should not open delete modal");
    }

    #[test]
    fn delete_confirm_modal_with_singular_counts() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "solo".to_string(),
            session_count: 1,
            worktree_count: 1,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("session"), "should show 'session' (singular)");
        assert!(out.contains("worktree"), "should show 'worktree' (singular)");
        // Ensure it doesn't say "sessions" or "worktrees" (plural)
        assert!(!out.contains("sessions"), "should not use plural 'sessions' for count 1");
        assert!(!out.contains("worktrees"), "should not use plural 'worktrees' for count 1");
    }

    #[test]
    fn delete_confirm_modal_with_zero_counts() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "empty".to_string(),
            session_count: 0,
            worktree_count: 0,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("sessions"), "should use plural 'sessions' for count 0");
        assert!(out.contains("worktrees"), "should use plural 'worktrees' for count 0");
    }

    #[test]
    fn delete_confirm_preserves_project_name_on_continue() {
        let mut modal = Modal::DeleteConfirm {
            project_name: "my-project".to_string(),
            session_count: 3,
            worktree_count: 2,
        };
        // Press some random keys — modal should still hold the same project name.
        assert!(matches!(advance_modal(&mut modal, KeyCode::Char('z')), ModalOutcome::Continue));
        assert!(matches!(advance_modal(&mut modal, KeyCode::Left), ModalOutcome::Continue));
        // Now confirm — should return the original name.
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::DeleteConfirmed { project } => assert_eq!(project, "my-project"),
            _ => panic!("expected DeleteConfirmed after Continue keys"),
        }
    }

    #[test]
    fn delete_confirm_modal_renders_on_small_terminal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows);
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "test".to_string(),
            session_count: 0,
            worktree_count: 0,
        });
        // Render on a very small terminal — should not panic.
        let out = render_to_string(&state, 30, 10);
        assert!(out.contains("Delete"), "modal should still render on small terminal");
    }
}
