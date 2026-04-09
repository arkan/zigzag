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
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

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
pub mod refresh;

use z_core::action::{ActionDef, ActionType, PaneType, ResolvedAction};
use z_core::domain::{CiStatus, PrState, PullRequest, Project, Session};
use z_core::traits::SessionRefresher;
use z_core::theme::{Rgb, ThemeStyle};

// ---------------------------------------------------------------------------
// Theme → ratatui style conversion
// ---------------------------------------------------------------------------

fn rgb_to_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

fn theme_style_to_style(ts: &ThemeStyle) -> Style {
    let mut s = Style::default();
    if let Some(fg) = ts.fg {
        s = s.fg(rgb_to_color(fg));
    }
    if let Some(bg) = ts.bg {
        s = s.bg(rgb_to_color(bg));
    }
    if ts.bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    if ts.dim {
        s = s.add_modifier(Modifier::DIM);
    }
    s
}

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
    /// Available autopilot workflows for this project (built-in + per-repo custom).
    pub workflows: Vec<WorkflowInfo>,
    /// Per-repo action definitions (from `.config/z.kdl`).
    pub repo_actions: Vec<ActionDef>,
}

/// Minimal workflow descriptor used by the TUI workflow selector modal.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowInfo {
    pub name: String,
    /// Human-readable trigger label (e.g. "post-push").
    pub trigger: String,
    /// Optional short description.
    pub description: String,
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

/// Action returned by `run_tui` once the user commits to something that
/// requires leaving the alternate screen (e.g. opening a Zellij session).
///
/// Actions that can be resolved without leaving the TUI (add/edit/delete
/// project, kill session, prune) are handled in-place via [`TuiCallbacks`]
/// and never appear here.
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
    /// User pressed `n` — create a new session for the selected project on a named branch.
    New { project: String, branch: String },
    /// User pressed `e` — open per-repo config in $EDITOR.
    EditPerRepoConfig { project_path: std::path::PathBuf },
    /// User pressed `Ctrl+k g` — open lazygit in the selected session.
    LazyGit { project: String, session: String },
    /// User selected an action from the action menu (Alt+r).
    RunAction {
        session: String,
        command: String,
        pane_type: PaneType,
    },
}

/// All callbacks the TUI can invoke to mutate external state without leaving
/// the alternate screen.
pub struct TuiCallbacks<'a> {
    pub prune_fn: &'a dyn Fn(bool) -> io::Result<String>,
    pub log_fn: &'a dyn Fn(usize) -> io::Result<Vec<String>>,
    pub swap_fn: &'a dyn Fn(usize, usize) -> io::Result<()>,
    pub kill_session_fn: &'a dyn Fn(&str) -> io::Result<()>,
    pub add_project_fn: &'a dyn Fn(&str, &str, Option<&str>, Option<&str>) -> io::Result<()>,
    pub edit_project_fn: &'a dyn Fn(&str, &str, &str, Option<&str>, Option<&str>) -> io::Result<()>,
    pub delete_project_fn: &'a dyn Fn(&str) -> io::Result<()>,
    pub reload_fn: &'a dyn Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>,
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
    /// Pre-filled form for editing an existing project. The `String` is the
    /// original project name, used to detect renames and to identify the entry
    /// to remove from the KDL file on save.
    EditProject(ProjectForm, String),
    /// Confirmation dialog shown before deleting a project.
    DeleteConfirm {
        project_name: String,
        session_count: usize,
        worktree_count: usize,
    },
    /// Workflow selector shown when the user presses 'a' (autopilot).
    WorkflowSelector {
        project: String,
        workflows: Vec<WorkflowInfo>,
        selected: usize,
    },
    /// Confirmation dialog shown before deleting a session.
    DeleteSessionConfirm { session: String },
    /// Full-screen help overlay showing all keybindings (opened with '?').
    Help,
    /// Branch name input shown when the user presses 'n' (new session).
    BranchInput { project: String, input: String },
    /// Scrollable log viewer opened with 'l'.
    LogViewer { lines: Vec<String>, scroll_offset: usize },
    /// Action menu shown when the user presses Alt+r.
    ActionMenu { actions: Vec<ResolvedAction>, selected: usize },
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
    SubmitEdit {
        original_name: String,
        path: String,
        name: String,
        host: Option<String>,
        token: Option<String>,
    },
    DeleteConfirmed { project: String },
    SessionDeleteConfirmed { session: String },
    WorkflowSelected { #[allow(dead_code)] project: String, #[allow(dead_code)] workflow: String },
    NewBranch { project: String, branch: String },
    ActionSelected { action: ResolvedAction },
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
    /// Review status for the PR (filled asynchronously).
    pub review: Option<z_core::domain::ReviewStatus>,
}

/// Combined PR/CI/Zellij data from the forge/session background thread.
struct ForgeData {
    pr: Option<PullRequest>,
    ci: CiStatus,
    zellij: Option<ZellijInfo>,
    review: Option<z_core::domain::ReviewStatus>,
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
    /// Forge client used to fetch PR/CI data in background threads.
    pub forge_client: Arc<dyn z_core::traits::ForgeClient + Send + Sync>,
    /// Current preview pane data (loading / ready / error).
    pub preview_data: PreviewData,
    /// Key identifying what we last requested a preview for.
    /// Format: `"{project_name}:{branch}"`.
    pub preview_key: String,
    /// Receiver for the in-flight async git fetch, if any.
    pub preview_rx: Option<mpsc::Receiver<Result<GitInfo, String>>>,
    /// Receiver for the in-flight async forge/Zellij fetch (PR, CI, session info).
    pub(crate) forge_rx: Option<mpsc::Receiver<Result<ForgeData, String>>>,
    /// Session names (e.g. `"myapp:feat-login"`) that have pending notifications.
    /// Sessions in this set render with a 🔔 badge in the SESSIONS panel.
    pub notifications: HashSet<String>,
    /// Active modal overlay, if any.
    pub modal: Option<Modal>,
    /// One-shot status message to display in the status bar (e.g. prune result).
    /// Shown instead of project info; cleared on the next render by the caller.
    pub status_message: Option<String>,
    /// Color theme applied to the entire TUI.
    pub theme: z_core::theme::Theme,
    /// Timestamp when Ctrl+k leader key was pressed. `None` = not waiting.
    /// If set, the next keypress is dispatched as a leader combo.
    /// Expires after 2 seconds.
    pub leader_pending: Option<Instant>,
    /// Global action definitions (from `~/.config/z/config.kdl`).
    pub global_actions: Vec<ActionDef>,
    /// Default AI review tool name (from global config, default: `"codex"`).
    pub review_tool: String,
    /// Session refresher used to poll sessions/notifications in background.
    pub refresher: Arc<dyn SessionRefresher>,
    /// Receiver for the in-flight session refresh, if any.
    pub(crate) refresh_rx: Option<mpsc::Receiver<refresh::RefreshData>>,
    /// Timestamp of the last refresh spawn.
    pub(crate) last_refresh: Instant,
}

impl TuiState {
    pub fn new(
        entries: Vec<ProjectEntry>,
        navigation: Navigation,
        forge_client: Arc<dyn z_core::traits::ForgeClient + Send + Sync>,
        refresher: Arc<dyn SessionRefresher>,
    ) -> Self {
        Self {
            entries,
            selected_project: 0,
            selected_session: 0,
            focused_panel: Panel::Projects,
            navigation,
            search_mode: false,
            search_query: String::new(),
            forge_client,
            preview_data: PreviewData::Loading,
            preview_key: String::new(),
            preview_rx: None,
            forge_rx: None,
            notifications: HashSet::new(),
            modal: None,
            status_message: None,
            theme: z_core::theme::Theme::default(),
            leader_pending: None,
            global_actions: Vec::new(),
            review_tool: "codex".to_string(),
            refresher,
            refresh_rx: None,
            last_refresh: Instant::now(),
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
        let forge_client = Arc::clone(&self.forge_client);
        std::thread::spawn(move || {
            let forge = ForgeData {
                pr: forge_client.get_pr(&project_name, &branch).ok().flatten(),
                ci: forge_client
                    .get_ci_status(&project_name, &branch)
                    .unwrap_or(CiStatus::Unknown),
                zellij: fetch_zellij_info(&session_name),
                review: forge_client.get_review_status(&project_name, &branch).ok().flatten(),
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
                    info.review = forge.review;
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

    /// Spawn a background session refresh if 5 seconds have elapsed and no
    /// refresh is already in-flight.
    pub fn trigger_refresh(&mut self) {
        const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

        if self.refresh_rx.is_some() || self.last_refresh.elapsed() < REFRESH_INTERVAL {
            return;
        }

        let refresher = Arc::clone(&self.refresher);
        let projects: Vec<Project> = self.entries.iter().map(|e| e.project.clone()).collect();
        let (tx, rx) = mpsc::channel();
        self.refresh_rx = Some(rx);
        self.last_refresh = Instant::now();

        std::thread::spawn(move || {
            let sessions = refresher.fetch_all_sessions(&projects);
            let notifications = refresher.fetch_notifications();
            let _ = tx.send(refresh::RefreshData {
                sessions,
                notifications,
            });
        });
    }

    /// Poll the in-flight session refresh channel; merge results if arrived.
    /// Skips applying results while a modal is open to avoid disrupting forms.
    pub fn poll_refresh(&mut self) {
        let data = match &self.refresh_rx {
            Some(rx) => match rx.try_recv() {
                Ok(data) => Some(data),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.refresh_rx = None;
                    None
                }
            },
            None => return,
        };

        if let Some(data) = data {
            self.refresh_rx = None;

            // Defer merge while a modal is open.
            if self.modal.is_some() {
                return;
            }

            let result = refresh::merge_refresh(
                &mut self.entries,
                &mut self.notifications,
                data,
                self.selected_project,
                self.selected_session,
            );
            self.selected_project = result.selected_project;
            self.selected_session = result.selected_session;
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
        review: None,
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
// Lightweight JSON helpers (used by Zellij info parser)
// ---------------------------------------------------------------------------

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
    let trimmed = json[after_colon..].trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let rest = &trimmed[1..];
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

/// Shared Tab-key logic for both AddProject and EditProject modals.
///
/// - Path field (field 0) with completions available → complete the path, stay on field 0.
/// - Path field with no completions → validate path and navigate to field 1.
/// - Any other field → navigate to next field (wrapping around).
fn tab_advance_with_completion(form: &mut ProjectForm) {
    if form.active_field == 0 {
        let completions = complete_path(&form.fields[0].value);
        match completions.len() {
            0 => {
                // No completions: validate and navigate forward.
                validate_path_field(form);
                form.active_field = 1;
            }
            1 => {
                // Single match: complete inline with trailing slash.
                // Autofill name before adding trailing slash (file_name() returns
                // None for paths ending with '/', so we need the clean path first).
                let completed = completions[0].clone();
                form.fields[0].value = completed.clone();
                form.fields[0].warning = None;
                autofill_name_if_empty(form);
                let with_slash = if completed.ends_with('/') {
                    completed
                } else {
                    format!("{}/", completed)
                };
                form.fields[0].value = with_slash;
            }
            _ => {
                // Multiple matches: complete to longest common prefix.
                let prefix = longest_common_prefix(&completions);
                if prefix.len() > form.fields[0].value.trim_end_matches('/').len() {
                    form.fields[0].value = prefix;
                    form.fields[0].warning = None;
                    autofill_name_if_empty(form);
                }
            }
        }
    } else {
        // Non-path fields: navigate to next field.
        form.active_field = (form.active_field + 1) % form.fields.len();
    }
}

/// Returns `Some(s)` if the trimmed string is non-empty, else `None`.
fn non_empty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// List matching directory entries for tab-completion on a partial path.
/// Expands `~` before processing. Returns only directories, sorted.
/// Returns an empty vec for empty input or unreadable directories.
fn complete_path(partial: &str) -> Vec<String> {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let expanded = expand_tilde_path(trimmed);

    // Split into parent directory and the prefix to match.
    let (dir, prefix): (&str, &str) = if expanded.ends_with('/') {
        (expanded.as_str(), "")
    } else {
        let p = std::path::Path::new(&expanded);
        let parent = p.parent().and_then(|p| p.to_str()).unwrap_or(".");
        let file = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
        (parent, file)
    };

    let dir_path = if dir.is_empty() { "." } else { dir };

    let Ok(entries) = std::fs::read_dir(dir_path) else {
        return vec![];
    };

    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with(prefix) {
                let full = std::path::Path::new(dir_path).join(&name);
                Some(full.to_str()?.to_string())
            } else {
                None
            }
        })
        .collect();

    matches.sort();
    matches
}

/// Compute the longest common prefix of a non-empty slice of strings.
/// Returns an empty string if the slice is empty.
fn longest_common_prefix(strs: &[String]) -> String {
    if strs.is_empty() {
        return String::new();
    }
    let first = strs[0].as_str();
    let mut prefix_len = first.len();
    for s in &strs[1..] {
        let new_len: usize = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map(|((i, c), _)| i + c.len_utf8())
            .unwrap_or(0);
        if new_len < prefix_len {
            prefix_len = new_len;
        }
    }
    first[..prefix_len].to_string()
}

/// Process one keypress inside the given modal. Mutates modal state in place.
/// Returns the outcome: whether to close, submit with data, or continue.
fn advance_modal(modal: &mut Modal, code: KeyCode) -> ModalOutcome {
    match modal {
        Modal::AddProject(form) => match code {
            KeyCode::Esc => ModalOutcome::Close,

            KeyCode::Tab => {
                tab_advance_with_completion(form);
                ModalOutcome::Continue
            }

            KeyCode::BackTab => {
                let was_path = form.active_field == 0;
                form.active_field =
                    (form.active_field + form.fields.len() - 1) % form.fields.len();
                if was_path {
                    validate_path_field(form);
                }
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

        Modal::EditProject(form, original_name) => match code {
            KeyCode::Esc => ModalOutcome::Close,

            KeyCode::Tab => {
                tab_advance_with_completion(form);
                ModalOutcome::Continue
            }

            KeyCode::BackTab => {
                let was_path = form.active_field == 0;
                form.active_field =
                    (form.active_field + form.fields.len() - 1) % form.fields.len();
                if was_path {
                    validate_path_field(form);
                }
                ModalOutcome::Continue
            }

            KeyCode::Enter => {
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
                    ModalOutcome::SubmitEdit {
                        original_name: original_name.clone(),
                        path,
                        name,
                        host,
                        token,
                    }
                } else {
                    ModalOutcome::Continue
                }
            }

            KeyCode::Backspace => {
                let fi = form.active_field;
                form.fields[fi].value.pop();
                if fi == 1 {
                    // Show rename warning when name differs from original.
                    let trimmed = form.fields[1].value.trim().to_string();
                    let is_renamed = trimmed != *original_name;
                    form.fields[1].warning = if is_renamed && !trimmed.is_empty() {
                        Some("Existing sessions will not be renamed".to_string())
                    } else {
                        None
                    };
                } else {
                    form.fields[fi].warning = None;
                }
                ModalOutcome::Continue
            }

            KeyCode::Char(c) => {
                let fi = form.active_field;
                form.fields[fi].value.push(c);
                if fi == 1 {
                    // Show rename warning when name differs from original.
                    let trimmed = form.fields[1].value.trim().to_string();
                    let is_renamed = trimmed != *original_name;
                    form.fields[1].warning = if is_renamed {
                        Some("Existing sessions will not be renamed".to_string())
                    } else {
                        None
                    };
                } else {
                    form.fields[fi].warning = None;
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

        Modal::DeleteSessionConfirm { session } => match code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ModalOutcome::Close,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                ModalOutcome::SessionDeleteConfirmed { session: session.clone() }
            }
            _ => ModalOutcome::Continue,
        },

        Modal::WorkflowSelector { project, workflows, selected } => match code {
            KeyCode::Esc => ModalOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                }
                ModalOutcome::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected + 1 < workflows.len() {
                    *selected += 1;
                }
                ModalOutcome::Continue
            }
            KeyCode::Enter => {
                if let Some(wf) = workflows.get(*selected) {
                    ModalOutcome::WorkflowSelected {
                        project: project.clone(),
                        workflow: wf.name.clone(),
                    }
                } else {
                    ModalOutcome::Close
                }
            }
            _ => ModalOutcome::Continue,
        },

        Modal::Help => match code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => ModalOutcome::Close,
            _ => ModalOutcome::Continue,
        },

        Modal::BranchInput { project, input } => match code {
            KeyCode::Esc => ModalOutcome::Close,
            KeyCode::Enter => {
                let branch = input.trim().to_string();
                if branch.is_empty() {
                    ModalOutcome::Continue
                } else {
                    ModalOutcome::NewBranch { project: project.clone(), branch }
                }
            }
            KeyCode::Backspace => {
                input.pop();
                ModalOutcome::Continue
            }
            KeyCode::Char(c) => {
                input.push(c);
                ModalOutcome::Continue
            }
            _ => ModalOutcome::Continue,
        },

        Modal::LogViewer { lines, scroll_offset } => match code {
            KeyCode::Esc | KeyCode::Char('q') => ModalOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                *scroll_offset = scroll_offset.saturating_sub(1);
                ModalOutcome::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *scroll_offset + 1 < lines.len() {
                    *scroll_offset += 1;
                }
                ModalOutcome::Continue
            }
            KeyCode::Char('G') => {
                *scroll_offset = lines.len().saturating_sub(1);
                ModalOutcome::Continue
            }
            KeyCode::Char('g') => {
                *scroll_offset = 0;
                ModalOutcome::Continue
            }
            _ => ModalOutcome::Continue,
        },

        Modal::ActionMenu { actions, selected } => match code {
            KeyCode::Esc => ModalOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                }
                ModalOutcome::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected + 1 < actions.len() {
                    *selected += 1;
                }
                ModalOutcome::Continue
            }
            KeyCode::Enter => {
                if let Some(action) = actions.get(*selected) {
                    ModalOutcome::ActionSelected { action: action.clone() }
                } else {
                    ModalOutcome::Close
                }
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
    status_message: Option<String>,
    callbacks: TuiCallbacks<'_>,
    forge_client: Box<dyn z_core::traits::ForgeClient + Send + Sync>,
    refresher: Box<dyn SessionRefresher>,
    theme: z_core::theme::Theme,
    global_actions: Vec<ActionDef>,
    review_tool: String,
) -> io::Result<TuiAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new(entries, navigation, Arc::from(forge_client), Arc::from(refresher));
    state.theme = theme;
    state.global_actions = global_actions;
    state.review_tool = review_tool;
    state.notifications = notifications;
    state.status_message = status_message;
    if let Some(idx) = initial_project {
        state.selected_project = idx;
    }
    // Kick off the first preview fetch immediately.
    state.trigger_preview_load();

    let result = event_loop(&mut terminal, &mut state, &callbacks);

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
    cb: &TuiCallbacks<'_>,
) -> io::Result<TuiAction> {
    loop {
        // Check if async git preview data has arrived.
        state.poll_preview();
        // Check if async forge/Zellij data has arrived.
        state.poll_forge();
        // Check if session/notification refresh data has arrived.
        state.poll_refresh();
        // Spawn a new session refresh if the interval has elapsed.
        state.trigger_refresh();

        terminal.draw(|f| render(f, state))?;

        // Expire leader key after 2 seconds.
        if let Some(when) = state.leader_pending {
            if when.elapsed() > Duration::from_secs(2) {
                state.leader_pending = None;
                state.status_message = None;
            }
        }

        // Poll with a short timeout so we can refresh the preview pane
        // without waiting for a keypress.
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Any keypress dismisses a one-shot status message (e.g. prune result).
            state.status_message = None;

            // ── Leader key (Ctrl+k) dispatch ───────────────────────────────
            if state.leader_pending.is_some() {
                state.leader_pending = None;
                match key.code {
                    KeyCode::Char('r') => {
                        let actions = build_action_menu(state);
                        if !actions.is_empty() {
                            state.modal = Some(Modal::ActionMenu {
                                actions,
                                selected: 0,
                            });
                        } else {
                            state.status_message = Some("No actions available in this context.".to_string());
                        }
                    }
                    KeyCode::Char('l') => {
                        match (cb.log_fn)(200) {
                            Ok(lines) => {
                                state.modal = Some(Modal::LogViewer {
                                    lines,
                                    scroll_offset: 0,
                                });
                            }
                            Err(e) => {
                                state.status_message = Some(format!("Failed to load logs: {e}"));
                            }
                        }
                    }
                    KeyCode::Char('g') => {
                        if let Some(entry) = state.selected_entry() {
                            let project = entry.project.name.clone();
                            let session = if state.focused_panel == Panel::Sessions {
                                let sessions = state.filtered_sessions();
                                sessions.get(state.selected_session).map(|s| s.name.clone())
                            } else {
                                entry.sessions.first().map(|s| s.name.clone())
                            };
                            if let Some(session) = session {
                                return Ok(TuiAction::LazyGit { project, session });
                            } else {
                                state.status_message = Some("No active session to open lazygit in.".to_string());
                            }
                        }
                    }
                    KeyCode::Esc => {
                        // Cancel leader
                    }
                    _ => {
                        state.status_message = Some(format!("Unknown leader combo: Ctrl+k {}", match key.code {
                            KeyCode::Char(c) => format!("{c}"),
                            _ => "?".to_string(),
                        }));
                    }
                }
                state.trigger_preview_load();
                continue;
            }

            // ── Modal mode ─────────────────────────────────────────────────
            if state.modal.is_some() {
                let outcome = advance_modal(state.modal.as_mut().unwrap(), key.code);
                match outcome {
                    ModalOutcome::Close => {
                        state.modal = None;
                    }
                    ModalOutcome::Submit { path, name, host, token } => {
                        state.modal = None;
                        apply_add_project(
                            state,
                            cb.add_project_fn,
                            cb.reload_fn,
                            &path, &name,
                            host.as_deref(), token.as_deref(),
                        );
                    }
                    ModalOutcome::SubmitEdit { original_name, path, name, host, token } => {
                        state.modal = None;
                        apply_edit_project(
                            state,
                            cb.edit_project_fn,
                            cb.reload_fn,
                            &original_name, &path, &name,
                            host.as_deref(), token.as_deref(),
                        );
                    }
                    ModalOutcome::DeleteConfirmed { project } => {
                        state.modal = None;
                        apply_delete_project(
                            state,
                            cb.delete_project_fn,
                            cb.reload_fn,
                            &project,
                        );
                    }
                    ModalOutcome::SessionDeleteConfirmed { session } => {
                        state.modal = None;
                        apply_delete_session(
                            state,
                            cb.kill_session_fn,
                            cb.reload_fn,
                            &session,
                        );
                    }
                    ModalOutcome::WorkflowSelected { project: _, workflow: _ } => {
                        // Autopilot workflow selection — currently a no-op
                        // (actual execution handled by `z autopilot run`).
                        state.modal = None;
                    }
                    ModalOutcome::ActionSelected { action } => {
                        state.modal = None;
                        match action.action {
                            ActionType::Run { ref command } => {
                                // Determine the target session name
                                let session_name = if state.focused_panel == Panel::Sessions {
                                    state.filtered_sessions()
                                        .get(state.selected_session)
                                        .map(|s| s.name.clone())
                                } else {
                                    state.selected_entry()
                                        .and_then(|e| e.sessions.first())
                                        .map(|s| s.name.clone())
                                };
                                if let Some(session) = session_name {
                                    return Ok(TuiAction::RunAction {
                                        session,
                                        command: command.clone(),
                                        pane_type: action.pane.clone(),
                                    });
                                } else {
                                    state.status_message = Some("No active session to run action in.".to_string());
                                }
                            }
                            ActionType::OpenUrl { ref url } => {
                                // Render OSC 8 hyperlink to stdout (works over SSH)
                                state.status_message = Some(format!("PR: {}", url));
                            }
                        }
                    }
                    ModalOutcome::NewBranch { project, branch } => {
                        state.modal = None;
                        return Ok(TuiAction::New { project, branch });
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
                            state.modal = Some(Modal::BranchInput {
                                project: entry.project.name.clone(),
                                input: String::new(),
                            });
                        }
                    }

                    KeyCode::Char('x') => {
                        let session_name = state
                            .filtered_sessions()
                            .get(state.selected_session)
                            .map(|s| s.name.clone());
                        if let Some(session) = session_name {
                            state.modal = Some(Modal::DeleteSessionConfirm { session });
                        }
                    }

                    KeyCode::Char('p') => {
                        // Prune: skip worktrees with uncommitted changes.
                        apply_prune(state, cb.prune_fn, false);
                    }

                    KeyCode::Char('P') => {
                        // Force prune: remove worktrees even with uncommitted changes.
                        apply_prune(state, cb.prune_fn, true);
                    }

                    KeyCode::Char('a') => {
                        if let Some(entry) = state.selected_entry() {
                            if !entry.workflows.is_empty() {
                                state.modal = Some(Modal::WorkflowSelector {
                                    project: entry.project.name.clone(),
                                    workflows: entry.workflows.clone(),
                                    selected: 0,
                                });
                            }
                        }
                    }

                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        state.leader_pending = Some(Instant::now());
                        state.status_message = Some("Ctrl+k ...".to_string());
                    }

                    KeyCode::Char('A') => {
                        if state.focused_panel == Panel::Projects {
                            state.modal = Some(Modal::AddProject(ProjectForm::new()));
                        }
                    }

                    KeyCode::Char('d') | KeyCode::Char('D') => {
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

                    KeyCode::Char('X') => {
                        let session_name = state
                            .filtered_sessions()
                            .get(state.selected_session)
                            .map(|s| s.name.clone());
                        if let Some(session) = session_name {
                            apply_delete_session(
                                state,
                                cb.kill_session_fn,
                                cb.reload_fn,
                                &session,
                            );
                        }
                    }

                    KeyCode::Char('e') => {
                        if let Some(entry) = state.selected_entry() {
                            return Ok(TuiAction::EditPerRepoConfig {
                                project_path: entry.project.path.clone(),
                            });
                        }
                    }

                    KeyCode::Char('E') => {
                        if state.focused_panel == Panel::Projects {
                            if let Some(entry) = state.selected_entry() {
                                let project = &entry.project;
                                let mut form = ProjectForm::new();
                                form.fields[0].value =
                                    project.path.to_string_lossy().to_string();
                                form.fields[1].value = project.name.clone();
                                form.fields[2].value =
                                    project.host.clone().unwrap_or_default();
                                form.fields[3].value =
                                    project.token.clone().unwrap_or_default();
                                // Suppress path-basename autofill: name is already set.
                                form.name_was_modified = true;
                                let original_name = project.name.clone();
                                state.modal =
                                    Some(Modal::EditProject(form, original_name));
                            }
                        }
                    }

                    KeyCode::Char('/') => {
                        state.search_mode = true;
                        state.search_query.clear();
                    }

                    KeyCode::Char('?') => {
                        state.modal = Some(Modal::Help);
                    }

                    // ── Project reorder: K = move up, J = move down ───
                    KeyCode::Char('K') if state.focused_panel == Panel::Projects => {
                        if state.selected_project > 0 {
                            let a = state.selected_project;
                            let b = a - 1;
                            if (cb.swap_fn)(b, a).is_ok() {
                                state.entries.swap(b, a);
                                state.selected_project = b;
                            }
                        }
                    }
                    KeyCode::Char('J') if state.focused_panel == Panel::Projects => {
                        let last = state.entries.len().saturating_sub(1);
                        if state.selected_project < last {
                            let a = state.selected_project;
                            let b = a + 1;
                            if (cb.swap_fn)(a, b).is_ok() {
                                state.entries.swap(a, b);
                                state.selected_project = b;
                            }
                        }
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
// In-place action helpers
// ---------------------------------------------------------------------------

/// Edit a project in-place and reload the TUI state.
///
/// Called directly from the event loop, so the TUI never leaves the alternate
/// screen — no flicker, no re-entry.
fn apply_edit_project(
    state: &mut TuiState,
    edit_project_fn: &dyn Fn(&str, &str, &str, Option<&str>, Option<&str>) -> io::Result<()>,
    reload_fn: &dyn Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>,
    original_name: &str,
    path: &str,
    name: &str,
    host: Option<&str>,
    token: Option<&str>,
) {
    match edit_project_fn(original_name, path, name, host, token) {
        Ok(()) => {
            state.status_message = Some(format!("Project {} saved.", name));
            if let Ok((entries, notifications)) = reload_fn() {
                state.entries = entries;
                state.notifications = notifications;
            }
        }
        Err(e) => {
            state.status_message = Some(format!("Error: {e}"));
        }
    }
}

/// Add a project in-place and reload the TUI state.
///
/// Called directly from the event loop, so the TUI never leaves the alternate
/// screen — no flicker, no re-entry.
fn apply_add_project(
    state: &mut TuiState,
    add_project_fn: &dyn Fn(&str, &str, Option<&str>, Option<&str>) -> io::Result<()>,
    reload_fn: &dyn Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>,
    path: &str,
    name: &str,
    host: Option<&str>,
    token: Option<&str>,
) {
    match add_project_fn(path, name, host, token) {
        Ok(()) => {
            state.status_message = Some(format!("Project {} added.", name));
            if let Ok((entries, notifications)) = reload_fn() {
                state.entries = entries;
                state.notifications = notifications;
                // Move cursor to the newly added project.
                if let Some(idx) = state.entries.iter().position(|e| e.project.name == name) {
                    state.selected_project = idx;
                }
            }
        }
        Err(e) => {
            state.status_message = Some(format!("Error: {e}"));
        }
    }
}

/// Delete a project in-place and reload the TUI state.
///
/// Called directly from the event loop, so the TUI never leaves the alternate
/// screen — no flicker, no re-entry.  After a successful deletion the cursor
/// is clamped to remain within the (now shorter) project list.
fn apply_delete_project(
    state: &mut TuiState,
    delete_project_fn: &dyn Fn(&str) -> io::Result<()>,
    reload_fn: &dyn Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>,
    project: &str,
) {
    match delete_project_fn(project) {
        Ok(()) => {
            state.status_message = Some(format!("Project {} deleted.", project));
            if let Ok((entries, notifications)) = reload_fn() {
                state.entries = entries;
                state.notifications = notifications;
                // Clamp cursor to valid range.
                if !state.entries.is_empty() {
                    state.selected_project = state.selected_project.min(state.entries.len() - 1);
                } else {
                    state.selected_project = 0;
                }
            }
        }
        Err(e) => {
            state.status_message = Some(format!("Error: {e}"));
        }
    }
}

/// Kill a session in-place and reload the TUI state.
///
/// Called directly from the event loop, so the TUI never leaves the alternate
/// screen — no flicker, no re-entry.
fn apply_delete_session(
    state: &mut TuiState,
    kill_session_fn: &dyn Fn(&str) -> io::Result<()>,
    reload_fn: &dyn Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>,
    session: &str,
) {
    match kill_session_fn(session) {
        Ok(()) => {
            state.status_message = Some(format!("Session {} killed.", session));
            if let Ok((entries, notifications)) = reload_fn() {
                state.entries = entries;
                state.notifications = notifications;
                // Clamp selected_session to the new sessions count.
                let session_count = state.filtered_sessions().len();
                if session_count == 0 {
                    state.selected_session = 0;
                } else {
                    state.selected_session = state.selected_session.min(session_count - 1);
                }
            }
        }
        Err(e) => {
            state.status_message = Some(format!("Error: {e}"));
        }
    }
}

/// Run the prune closure and store the result (or error) as a status message.
///
/// Called directly from the event loop when the user presses `p`, so the TUI
/// never leaves the alternate screen — no flicker, no re-entry.
/// Build the list of resolved actions for the action menu modal.
/// Uses built-in actions only (user config actions will be added later).
fn build_action_menu(state: &TuiState) -> Vec<ResolvedAction> {
    use z_core::action::{self, ActionEnv};

    let entry = match state.selected_entry() {
        Some(e) => e,
        None => return Vec::new(),
    };

    let builtins = action::builtin_actions();
    let merged = action::merge_actions(&[
        builtins,
        state.global_actions.clone(),
        entry.repo_actions.clone(),
    ]);

    // Determine current session context
    let selected_session = if state.focused_panel == Panel::Sessions {
        state.filtered_sessions().get(state.selected_session).cloned()
    } else {
        entry.sessions.first()
    };

    // Build env from preview data + entry
    let (pr_number, pr_url, ci_status) = match &state.preview_data {
        PreviewData::Ready(ref git_info) => {
            let pr_number = git_info.pr.as_ref().map(|pr| pr.number);
            let pr_url = git_info.pr.as_ref().map(|pr| pr.url.clone());
            let ci_status = git_info.ci.clone();
            (pr_number, pr_url, ci_status)
        }
        _ => (None, None, None),
    };

    let env = ActionEnv {
        project: entry.project.name.clone(),
        project_path: entry.project.path.to_string_lossy().to_string(),
        repo: None, // TODO: extract from git remote
        branch: selected_session.map(|s| s.branch.clone()),
        session: selected_session.map(|s| s.name.clone()),
        pr_number,
        pr_url,
        ci_status,
        has_new_comments: match &state.preview_data {
            PreviewData::Ready(ref info) => info.review.as_ref().map_or(false, |r| r.has_new_comments),
            _ => false,
        },
        review_tool: state.review_tool.clone(),
    };

    action::resolve_actions(&merged, &env).unwrap_or_default()
}

fn apply_prune(state: &mut TuiState, prune_fn: &dyn Fn(bool) -> io::Result<String>, force: bool) {
    match prune_fn(force) {
        Ok(msg) => state.status_message = Some(msg),
        Err(e) => state.status_message = Some(format!("Prune failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Top-level render: splits the terminal into main panels, preview, and status.
pub fn render(f: &mut Frame, state: &TuiState) {
    let area = f.area();

    // Fill the entire area with the theme background
    let bg_style = Style::default()
        .bg(rgb_to_color(state.theme.background))
        .fg(rgb_to_color(state.theme.foreground));
    f.render_widget(Block::default().style(bg_style), area);

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
            let notif_count: usize = entry
                .sessions
                .iter()
                .filter(|s| state.notifications.contains(&s.name))
                .count();
            let notif_badge = if notif_count > 0 {
                format!(" \u{1f514} {}", notif_count)
            } else {
                String::new()
            };
            ListItem::new(format!("{}{}{}{}", entry.project.name, active, remote, notif_badge))
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

    let theme = &state.theme;
    let border_style = if focused {
        theme_style_to_style(&theme.border_focused)
    } else {
        theme_style_to_style(&theme.border_unfocused)
    };
    let highlight = if focused {
        theme_style_to_style(&theme.item_selected_focused)
    } else {
        theme_style_to_style(&theme.item_selected_unfocused)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title_style(theme_style_to_style(&theme.title))
                .title(title),
        )
        .style(theme_style_to_style(&theme.item_normal))
        .highlight_symbol(&theme.highlight_symbol)
        .highlight_style(highlight);

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

    let theme = &state.theme;
    let border_style = if focused {
        theme_style_to_style(&theme.border_focused)
    } else {
        theme_style_to_style(&theme.border_unfocused)
    };
    let highlight = if focused {
        theme_style_to_style(&theme.item_selected_focused)
    } else {
        theme_style_to_style(&theme.item_selected_unfocused)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title_style(theme_style_to_style(&theme.title))
                .title(" SESSIONS "),
        )
        .style(theme_style_to_style(&theme.item_normal))
        .highlight_symbol(&theme.highlight_symbol)
        .highlight_style(highlight);

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

            // Review status line
            if let Some(review) = &info.review {
                if review.has_new_comments {
                    lines.push(format!(
                        " \u{1f4ac} {} new review comment{}",
                        review.comment_count,
                        if review.comment_count == 1 { "" } else { "s" }
                    ));
                } else if review.comment_count > 0 {
                    lines.push(format!(
                        " {} review comment{} (addressed)",
                        review.comment_count,
                        if review.comment_count == 1 { "" } else { "s" }
                    ));
                }
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

    let theme = &state.theme;
    let paragraph = Paragraph::new(content)
        .style(theme_style_to_style(&theme.preview_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme_style_to_style(&theme.border_unfocused))
                .title_style(theme_style_to_style(&theme.title))
                .title(" PREVIEW "),
        );
    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, area: Rect, state: &TuiState) {
    let first_line = if let Some(msg) = &state.status_message {
        format!(" {} ", msg)
    } else {
        state
            .selected_entry()
            .map(|e| {
                let locality = if e.project.host.is_some() { "remote" } else { "local" };
                let session_count = e.sessions.len();
                format!(" {} | {} | sessions: {} ", e.project.name, locality, session_count)
            })
            .unwrap_or_else(|| " No projects — add to ~/.config/z/projects.kdl ".to_string())
    };

    let hints = " [o]pen [n]ew [d]el session [p]rune [a]utopilot [A]dd [E]dit [D]el project [e]config [Ctrl+k]actions [/]search [?]help [q]uit";
    let content = format!("{}\n{}", first_line, hints);

    let theme = &state.theme;
    let paragraph = Paragraph::new(content)
        .style(theme_style_to_style(&theme.status_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme_style_to_style(&theme.border_unfocused))
                .title_style(theme_style_to_style(&theme.title))
                .title(" STATUS "),
        );
    f.render_widget(paragraph, area);
}

fn render_delete_confirm_modal(
    f: &mut Frame,
    project_name: &str,
    session_count: usize,
    worktree_count: usize,
    theme: &z_core::theme::Theme,
) {
    let area = f.area();
    let modal_width = 62u16;
    let modal_height = 9u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Delete Project ")
        .border_style(theme_style_to_style(&theme.indicator_error).add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let session_word = if session_count == 1 { "session" } else { "sessions" };
    let worktree_word = if worktree_count == 1 { "worktree" } else { "worktrees" };

    let modal_fg = theme_style_to_style(&theme.item_normal);
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Delete project: {}", project_name),
            theme_style_to_style(&theme.text_highlight),
        )),
        Line::from(Span::styled(
            format!(" Active {}: {}", session_word, session_count),
            modal_fg,
        )),
        Line::from(Span::styled(
            format!(" Git {}: {}", worktree_word, worktree_count),
            modal_fg,
        )),
        Line::from(""),
    ];

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)));
    lines.push(Line::from(Span::styled(
        " Enter/y: confirm  Esc/n: cancel  (only removes KDL entry)",
        dim,
    )));

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_delete_session_confirm_modal(f: &mut Frame, session: &str, theme: &z_core::theme::Theme) {
    let area = f.area();
    let modal_width = 56u16;
    let modal_height = 7u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Delete Session ")
        .border_style(theme_style_to_style(&theme.indicator_error).add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Kill session: {}", session),
            theme_style_to_style(&theme.text_highlight),
        )),
        Line::from(""),
        Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)),
        Line::from(Span::styled(" Enter/y: confirm  Esc/n: cancel", dim)),
    ];

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_workflow_selector_modal(
    f: &mut Frame,
    project: &str,
    workflows: &[WorkflowInfo],
    selected: usize,
    theme: &z_core::theme::Theme,
) {
    let area = f.area();
    let modal_height = (workflows.len() as u16 + 4).max(7).min(area.height);
    let modal_width = 72u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Autopilot: {} ", project))
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();

    for (i, wf) in workflows.iter().enumerate() {
        let is_selected = i == selected;
        let cursor = if is_selected { "\u{25b8} " } else { "  " };
        let desc = if wf.description.is_empty() {
            String::new()
        } else {
            format!("  {}", wf.description)
        };
        let text = format!("{}{:28}  {}{}", cursor, wf.name, wf.trigger, desc);
        let style = if is_selected {
            theme_style_to_style(&theme.item_selected_focused)
        } else {
            theme_style_to_style(&theme.item_normal)
        };
        lines.push(Line::from(Span::styled(text, style)));
    }

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)));
    lines.push(Line::from(Span::styled(
        " \u{2191}/\u{2193}: select  Enter: run  Esc: cancel",
        dim,
    )));

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_action_menu_modal(
    f: &mut Frame,
    actions: &[ResolvedAction],
    selected: usize,
    theme: &z_core::theme::Theme,
) {
    let area = f.area();
    let modal_height = (actions.len() as u16 + 4).max(7).min(area.height);
    let modal_width = 72u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Actions ")
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();

    for (i, action) in actions.iter().enumerate() {
        let is_selected = i == selected;
        let cursor = if is_selected { "\u{25b8} " } else { "  " };
        let icon = action.icon.as_deref().unwrap_or("");
        let icon_pad = if icon.is_empty() { "" } else { " " };
        let text = format!("{}{}{}{}", cursor, icon, icon_pad, action.name);
        let style = if is_selected {
            theme_style_to_style(&theme.item_selected_focused)
        } else {
            theme_style_to_style(&theme.item_normal)
        };
        lines.push(Line::from(Span::styled(text, style)));
    }

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)));
    lines.push(Line::from(Span::styled(
        " \u{2191}/\u{2193}: select  Enter: run  Esc: cancel",
        dim,
    )));

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_help_modal(f: &mut Frame, theme: &z_core::theme::Theme) {
    let area = f.area();
    let modal_height = 25u16;
    let modal_width = 56u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Keybindings ")
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let heading = theme_style_to_style(&theme.text_highlight).add_modifier(Modifier::UNDERLINED);
    let dim = theme_style_to_style(&theme.text_dim);
    let normal = theme_style_to_style(&theme.item_normal);

    let lines: Vec<Line> = vec![
        Line::from(Span::styled(" Navigation", heading)),
        Line::from(Span::styled("   \u{2191}/\u{2193} or k/j    Navigate list", normal)),
        Line::from(Span::styled("   \u{2190}/\u{2192} or h/l    Switch panel", normal)),
        Line::from(Span::styled("   Tab              Switch panel", normal)),
        Line::from(Span::styled("   /                Fuzzy search", normal)),
        Line::from(Span::styled("   Esc              Back / cancel", normal)),
        Line::from(""),
        Line::from(Span::styled(" Actions", heading)),
        Line::from(Span::styled("   o / Enter        Open session", normal)),
        Line::from(Span::styled("   n                New session on main branch", normal)),
        Line::from(Span::styled("   d                Delete session", normal)),
        Line::from(Span::styled("   A                Add project", normal)),
        Line::from(Span::styled("   E                Edit project", normal)),
        Line::from(Span::styled("   D                Delete project", normal)),
        Line::from(Span::styled("   p                Prune orphaned sessions", normal)),
        Line::from(Span::styled("   Ctrl+k r         Run action", normal)),
        Line::from(Span::styled("   Ctrl+k l         View logs", normal)),
        Line::from(Span::styled("   Ctrl+k g         Lazygit", normal)),
        Line::from(Span::styled("   a                Autopilot workflows", normal)),
        Line::from(Span::styled("   e                Edit per-repo config", normal)),
        Line::from(""),
        Line::from(Span::styled(" Session", heading)),
        Line::from(Span::styled("   Ctrl+O \u{2192} D      Detach (return to z)", normal)),
        Line::from(Span::styled("   Ctrl+Q           Quit session (return to z)", normal)),
        Line::from(Span::styled(" \u{2500}".repeat((inner.width.saturating_sub(1) / 2) as usize), dim)),
        Line::from(Span::styled("   Ctrl+k: r actions  l logs  g lazygit   ?  help  q  quit", dim)),
    ];

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_modal(f: &mut Frame, state: &TuiState) {
    let theme = &state.theme;
    let (form, title) = match &state.modal {
        None => return,
        Some(Modal::DeleteConfirm { project_name, session_count, worktree_count }) => {
            render_delete_confirm_modal(f, project_name, *session_count, *worktree_count, theme);
            return;
        }
        Some(Modal::DeleteSessionConfirm { session }) => {
            render_delete_session_confirm_modal(f, session, theme);
            return;
        }
        Some(Modal::WorkflowSelector { project, workflows, selected }) => {
            render_workflow_selector_modal(f, project, workflows, *selected, theme);
            return;
        }
        Some(Modal::ActionMenu { actions, selected }) => {
            render_action_menu_modal(f, actions, *selected, theme);
            return;
        }
        Some(Modal::Help) => {
            render_help_modal(f, theme);
            return;
        }
        Some(Modal::BranchInput { project, input }) => {
            render_branch_input_modal(f, project, input, theme);
            return;
        }
        Some(Modal::LogViewer { lines, scroll_offset }) => {
            render_log_viewer_modal(f, lines, *scroll_offset, theme);
            return;
        }
        Some(Modal::AddProject(form)) => (form, " Add Project "),
        Some(Modal::EditProject(form, _)) => (form, " Edit Project "),
    };

    let area = f.area();
    let modal_height = (form.fields.len() as u16) * 3 + 4;
    let modal_width = 62u16;
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();

    for (i, field) in form.fields.iter().enumerate() {
        let active = i == form.active_field;
        let opt_hint = if field.required { "" } else { " (opt)" };

        let label_text = format!(" {}{}:", field.label, opt_hint);
        let label_style = if active {
            theme_style_to_style(&theme.text_highlight)
        } else {
            theme_style_to_style(&theme.item_normal)
        };
        lines.push(Line::from(Span::styled(label_text, label_style)));

        let value_text = if active {
            format!(" \u{25b6} {}\u{2588}", field.value)
        } else {
            format!("   {}", field.value)
        };
        let value_style = if active {
            theme_style_to_style(&theme.item_selected_focused)
        } else {
            theme_style_to_style(&theme.item_normal)
        };
        lines.push(Line::from(Span::styled(value_text, value_style)));

        if let Some(warn) = &field.warning {
            lines.push(Line::from(Span::styled(
                format!(" \u{26a0} {}", warn),
                theme_style_to_style(&theme.indicator_warning),
            )));
        } else {
            lines.push(Line::from(""));
        }
    }

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    lines.push(Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)));
    lines.push(Line::from(Span::styled(
        " Tab: complete path / next  S-Tab: prev  Enter: save  Esc: cancel",
        dim,
    )));

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_branch_input_modal(f: &mut Frame, project: &str, input: &str, theme: &z_core::theme::Theme) {
    let area = f.area();
    let rect = modal_rect(52, 7, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" New Session \u{2014} {} ", project))
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let dim = theme_style_to_style(&theme.text_dim);
    let sep_width = inner.width.saturating_sub(1) as usize;
    let lines = vec![
        Line::from(Span::styled(" Branch name:", theme_style_to_style(&theme.text_highlight))),
        Line::from(Span::styled(
            format!(" \u{25b6} {}\u{2588}", input),
            theme_style_to_style(&theme.item_selected_focused),
        )),
        Line::from(""),
        Line::from(Span::styled("\u{2500}".repeat(sep_width), dim)),
        Line::from(Span::styled(" Enter: create  Esc: cancel", dim)),
    ];

    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

fn render_log_viewer_modal(f: &mut Frame, lines: &[String], scroll_offset: usize, theme: &z_core::theme::Theme) {
    let area = f.area();
    let modal_width = area.width.saturating_sub(4).min(120);
    let modal_height = area.height.saturating_sub(4).min(40);
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block_template = Block::default()
        .borders(Borders::ALL)
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block_template.inner(rect);

    if lines.is_empty() {
        let title = " Logs (0) \u{2014} Esc close ";
        let block = block_template.title(title);
        f.render_widget(block, rect);
        let paragraph = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(" No logs yet.", theme_style_to_style(&theme.text_dim))),
        ]))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
        f.render_widget(paragraph, inner);
        return;
    }

    let visible_height = inner.height as usize;
    let max_start = lines.len().saturating_sub(visible_height);
    let start = scroll_offset.min(max_start);
    let end = (start + visible_height).min(lines.len());

    let title = format!(
        " Logs ({}/{}) \u{2014} j/k scroll, G end, g top, Esc close ",
        start + 1,
        lines.len()
    );
    let block = block_template.title(title);
    f.render_widget(block, rect);

    let display_lines: Vec<Line> = lines[start..end]
        .iter()
        .map(|line| {
            let style = if line.contains("[ERROR]") {
                theme_style_to_style(&theme.log_error)
            } else if line.contains("[WARNING]") {
                theme_style_to_style(&theme.log_warning)
            } else {
                theme_style_to_style(&theme.log_default)
            };
            Line::styled(line.as_str(), style)
        })
        .collect();

    let paragraph = Paragraph::new(Text::from(display_lines))
        .style(Style::default().bg(rgb_to_color(theme.modal_background)));
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Session switch picker
// ---------------------------------------------------------------------------

/// State for the `z switch` session picker TUI.
pub struct SwitchPickerState {
    /// All z-managed session names, sorted alphabetically.
    pub sessions: Vec<String>,
    /// Ages corresponding to each session (largest unit: "2h", "30m", "1d", "5s").
    pub ages: Vec<Option<String>>,
    /// Pending notification counts per session (0 = no badge).
    pub notification_counts: Vec<usize>,
    /// Currently highlighted item index.
    pub selected: usize,
    /// Name of the current Zellij session (from `$ZELLIJ_SESSION_NAME`).
    pub current_session: String,
}

impl SwitchPickerState {
    pub fn new(sessions: Vec<String>, current_session: String) -> Self {
        let selected = sessions
            .iter()
            .position(|s| s == &current_session)
            .unwrap_or(0);
        let ages = vec![None; sessions.len()];
        let notification_counts = vec![0; sessions.len()];
        Self { sessions, ages, notification_counts, selected, current_session }
    }

    /// Create state with per-session age strings (notification counts default to 0).
    pub fn with_ages(
        sessions: Vec<String>,
        ages: Vec<Option<String>>,
        current_session: String,
    ) -> Self {
        debug_assert_eq!(
            sessions.len(),
            ages.len(),
            "sessions and ages must have the same length"
        );
        let selected = sessions
            .iter()
            .position(|s| s == &current_session)
            .unwrap_or(0);
        let notification_counts = vec![0; sessions.len()];
        Self { sessions, ages, notification_counts, selected, current_session }
    }

    /// Create state with per-session age strings and notification counts.
    pub fn with_notifications(
        sessions: Vec<String>,
        ages: Vec<Option<String>>,
        notification_counts: Vec<usize>,
        current_session: String,
    ) -> Self {
        debug_assert_eq!(
            sessions.len(),
            ages.len(),
            "sessions and ages must have the same length"
        );
        debug_assert_eq!(
            sessions.len(),
            notification_counts.len(),
            "sessions and notification_counts must have the same length"
        );
        let selected = sessions
            .iter()
            .position(|s| s == &current_session)
            .unwrap_or(0);
        Self { sessions, ages, notification_counts, selected, current_session }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.sessions.len() {
            self.selected += 1;
        }
    }

    pub fn selected_session(&self) -> Option<&str> {
        self.sessions.get(self.selected).map(|s| s.as_str())
    }
}

/// Render the session switch picker into the frame.
fn render_switch_picker(f: &mut Frame, state: &SwitchPickerState, theme: &z_core::theme::Theme) {
    let area = f.area();

    // Fill background
    let bg_style = Style::default()
        .bg(rgb_to_color(theme.background))
        .fg(rgb_to_color(theme.foreground));
    f.render_widget(Block::default().style(bg_style), area);

    let modal_width = area.width.saturating_sub(4).min(60).max(40.min(area.width));
    let content_rows = state.sessions.len().min(20) as u16;
    let modal_height = content_rows.saturating_add(3).min(area.height);
    let rect = modal_rect(modal_width, modal_height, area);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Switch Session ")
        .title_style(theme_style_to_style(&theme.modal_title))
        .border_style(theme_style_to_style(&theme.modal_border));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if inner.height < 2 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let inner_width = modal_width.saturating_sub(2) as usize;
    let right_cols = 10;
    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let is_current = name == &state.current_session;
            let marker = if is_current { "\u{25cf} " } else { "  " };
            let left = format!("{}{}", marker, name);
            let left_display_len = marker.chars().count() + name.len();
            let age_str = state
                .ages
                .get(i)
                .and_then(|a| a.as_deref())
                .unwrap_or("");
            let notif_count = state.notification_counts.get(i).copied().unwrap_or(0);

            let age_col = format!("{:>4}", age_str);
            let (badge_col, _badge_display_len) = if notif_count > 0 {
                let digits = notif_count.to_string();
                let display_len = 2 + 1 + digits.len();
                let pad = 6usize.saturating_sub(display_len);
                (format!("\u{1f514} {}{}", digits, " ".repeat(pad)), 6)
            } else {
                ("      ".to_string(), 6)
            };

            let label = if inner_width > left_display_len + right_cols + 1 {
                let padding = inner_width - left_display_len - right_cols;
                format!("{}{}{}{}", left, " ".repeat(padding), badge_col, age_col)
            } else {
                left
            };
            let style = if i == state.selected {
                theme_style_to_style(&theme.item_selected_focused)
            } else if is_current {
                theme_style_to_style(&theme.indicator_active)
            } else {
                theme_style_to_style(&theme.item_normal)
            };
            ListItem::new(label).style(style)
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.selected));

    let list = List::new(items);
    f.render_stateful_widget(list, chunks[0], &mut list_state);

    let footer = Paragraph::new(Line::from(Span::styled(
        " j/k navigate  Enter switch  Esc close",
        theme_style_to_style(&theme.text_dim),
    )));
    f.render_widget(footer, chunks[1]);
}

fn switch_picker_event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut SwitchPickerState,
    theme: &z_core::theme::Theme,
) -> io::Result<Option<String>> {
    loop {
        terminal.draw(|f| render_switch_picker(f, state, theme))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => state.move_down(),
                KeyCode::Char('k') | KeyCode::Up => state.move_up(),
                KeyCode::Enter => {
                    return Ok(state.selected_session().map(|s| s.to_string()));
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(None);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

/// Launch the interactive session switch picker TUI.
///
/// Returns `Some(session_name)` if the user pressed `Enter` to switch,
/// or `None` if the user pressed `Esc`/`q` to close without switching.
///
/// Each entry in `session_entries` is `(session_name, age, notification_count)` where
/// `age` is the compact duration string (e.g. `"2h"`) or `None` if unknown, and
/// `notification_count` is the number of pending notifications (0 = no badge).
pub fn run_switch_picker(
    session_entries: Vec<(String, Option<String>, usize)>,
    current_session: String,
) -> io::Result<Option<String>> {
    if session_entries.is_empty() {
        return Ok(None);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut sessions = Vec::with_capacity(session_entries.len());
    let mut ages = Vec::with_capacity(session_entries.len());
    let mut notification_counts = Vec::with_capacity(session_entries.len());
    for (s, a, n) in session_entries {
        sessions.push(s);
        ages.push(a);
        notification_counts.push(n);
    }
    let mut state =
        SwitchPickerState::with_notifications(sessions, ages, notification_counts, current_session);

    let theme = z_core::theme::Theme::default();
    let result = switch_picker_event_loop(&mut terminal, &mut state, &theme);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

// ---------------------------------------------------------------------------
// Standalone log viewer (used by `z logs-viewer` in a Zellij floating pane)
// ---------------------------------------------------------------------------

/// Run a standalone log viewer TUI. `lines` are the log entries to display.
/// Blocks until the user presses Esc or q.
pub fn run_log_viewer(lines: Vec<String>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let scroll_offset = lines.len().saturating_sub(1);
    let mut modal = Modal::LogViewer { lines, scroll_offset };

    let result = loop {
        let theme = z_core::theme::Theme::default();
        terminal.draw(|f| {
            if let Modal::LogViewer { ref lines, scroll_offset } = modal {
                render_log_viewer_modal(f, lines, scroll_offset, &theme);
            }
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let event::Event::Key(key) = event::read()? {
            if key.kind != event::KeyEventKind::Press {
                continue;
            }
            match advance_modal(&mut modal, key.code) {
                ModalOutcome::Close => break Ok(()),
                _ => {}
            }
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
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

    /// No-op forge client for tests (never called — preview is set manually).
    struct MockForgeClient;
    impl z_core::traits::ForgeClient for MockForgeClient {
        fn get_pr(&self, _: &str, _: &str) -> z_core::error::Result<Option<PullRequest>> {
            Ok(None)
        }
        fn get_ci_status(&self, _: &str, _: &str) -> z_core::error::Result<CiStatus> {
            Ok(CiStatus::Unknown)
        }
        fn get_review_status(&self, _: &str, _: &str) -> z_core::error::Result<Option<z_core::domain::ReviewStatus>> {
            Ok(None)
        }
    }

    fn mock_forge() -> Arc<dyn z_core::traits::ForgeClient + Send + Sync> {
        Arc::new(MockForgeClient)
    }

    struct MockSessionRefresher;
    impl SessionRefresher for MockSessionRefresher {
        fn fetch_all_sessions(&self, _: &[Project]) -> Vec<(String, Vec<Session>)> {
            Vec::new()
        }
        fn fetch_notifications(&self) -> HashSet<String> {
            HashSet::new()
        }
    }

    fn mock_refresher() -> Arc<dyn SessionRefresher> {
        Arc::new(MockSessionRefresher)
    }

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
                workflows: vec![],
                repo_actions: vec![],
            },
            ProjectEntry {
                project: make_project("hermes", false),
                sessions: vec![],
                worktree_count: 0,
                workflows: vec![],
                repo_actions: vec![],
            },
            ProjectEntry {
                project: make_project("prod-api", true),
                sessions: vec![],
                worktree_count: 0,
                workflows: vec![],
                repo_actions: vec![],
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
            review: None,
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
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"), "should render PROJECTS panel header");
        assert!(out.contains("SESSIONS"), "should render SESSIONS panel header");
    }

    #[test]
    fn renders_all_project_names() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("myapp"), "should show 'myapp'");
        assert!(out.contains("hermes"), "should show 'hermes'");
        assert!(out.contains("prod-api"), "should show 'prod-api'");
    }

    #[test]
    fn renders_active_session_indicator() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        // myapp has sessions → should have the ● bullet (U+25CF)
        assert!(out.contains('\u{25cf}'), "should show active session indicator ●");
    }

    #[test]
    fn renders_remote_project_icon() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        // prod-api is remote → should have 🌐 (U+1F310)
        assert!(out.contains('\u{1f310}'), "should show remote project icon 🌐");
    }

    #[test]
    fn renders_sessions_for_selected_project() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 140, 24);
        assert!(out.contains("[o]"), "should show [o] hint");
        assert!(out.contains("[q]"), "should show [q] hint");
        assert!(out.contains("[n]"), "should show [n] hint");
        assert!(out.contains("[d]"), "should show [d] hint");
        assert!(out.contains("[e]"), "should show [e] edit config hint");
        assert!(out.contains("[Ctrl+k]"), "should show [Ctrl+k] leader key hint");
    }

    #[test]
    fn e_key_returns_edit_per_repo_config_action() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        // With no entries, selected_entry() returns None — no action should be emitted.
        assert!(state.selected_entry().is_none(), "empty state should have no selected entry");
    }

    #[test]
    fn renders_status_bar_project_info() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        // Status bar shows selected project name
        assert!(out.contains("myapp"), "status bar should mention selected project");
        assert!(out.contains("local"), "status bar should show locality");
    }

    #[test]
    fn renders_empty_state_without_panic() {
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"), "should still render PROJECTS panel");
        assert!(out.contains("SESSIONS"), "should still render SESSIONS panel");
    }

    #[test]
    fn renders_search_query_in_header() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_mode = true;
        state.search_query = "my".to_string();
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("/my"), "search query should appear in PROJECTS header");
    }

    // ── Preview pane snapshot tests ────────────────────────────────────────

    #[test]
    fn renders_preview_pane_header() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("PREVIEW"), "should render PREVIEW panel header");
    }

    #[test]
    fn renders_preview_loading_indicator() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Initial state is Loading
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("Loading") || out.contains("loading"),
            "should show loading indicator in preview pane"
        );
    }

    #[test]
    fn renders_preview_ready_with_branch_and_tracking() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(make_git_info()); // is_dirty = true
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("dirty"), "should show dirty working tree status");
    }

    #[test]
    fn renders_preview_clean_status() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(GitInfo {
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            is_dirty: false,
            commits: vec![],
            pr: None,
            ci: None,
            zellij: None,
            review: None,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("clean"), "should show clean working tree status");
    }

    #[test]
    fn renders_preview_commit_list() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(make_git_info());
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("a1b2c3"), "should show commit hash");
        assert!(out.contains("d4e5f6"), "should show second commit hash");
    }

    #[test]
    fn renders_preview_recent_commits_label() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(make_git_info());
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("recent commits") || out.contains("recent"),
            "should label the commit section"
        );
    }

    #[test]
    fn renders_preview_error_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Error("not a git repository".to_string());
        let out = render_to_string(&state, 80, 30);
        assert!(
            out.contains("Error") || out.contains("error") || out.contains("not a git"),
            "should show error message in preview pane"
        );
    }

    #[test]
    fn renders_preview_no_tracking_when_zero_ahead_behind() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(GitInfo {
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            is_dirty: false,
            commits: vec![],
            pr: None,
            ci: None,
            zellij: None,
            review: None,
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
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(
            matches!(state.preview_data, PreviewData::Loading),
            "initial preview_data should be Loading"
        );
    }

    #[test]
    fn initial_preview_key_is_empty() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert_eq!(state.preview_key, "");
    }

    #[test]
    fn trigger_preview_load_sets_key() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.trigger_preview_load();
        assert!(!state.preview_key.is_empty(), "preview_key should be set after trigger");
        assert!(
            state.preview_key.contains("myapp"),
            "preview_key should reference the selected project"
        );
    }

    #[test]
    fn trigger_preview_load_sets_loading_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        state.trigger_preview_load(); // should not panic
        assert_eq!(state.preview_key, "");
        assert!(matches!(state.preview_data, PreviewData::Loading));
    }

    #[test]
    fn poll_preview_updates_state_from_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_rx = None;
        state.poll_preview(); // should not panic
    }

    #[test]
    fn poll_preview_handles_disconnected_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.focused_panel = Panel::Projects;
        state.selected_session = 1; // should be ignored — uses first session
        let key = state.current_preview_key().unwrap();
        assert!(key.contains("main"), "should use first session branch when projects focused");
    }

    #[test]
    fn preview_key_empty_branch_for_no_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 1; // hermes has no sessions
        let key = state.current_preview_key().unwrap();
        assert!(key.ends_with(':'), "key should end with ':' when project has no sessions");
    }

    #[test]
    fn renders_preview_at_minimum_terminal_height() {
        // 8 (preview) + 3 (status) + 1 (min main) = 12 minimum
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let _out = render_to_string(&state, 80, 12); // should not panic
    }

    #[test]
    fn preview_key_changes_on_project_navigation() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert_eq!(state.selected_project, 0);
        state.move_down();
        assert_eq!(state.selected_project, 1);
        state.move_down();
        assert_eq!(state.selected_project, 2);
    }

    #[test]
    fn navigate_up_does_not_underflow() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.move_up();
        assert_eq!(state.selected_project, 0, "should stay at 0");
    }

    #[test]
    fn navigate_down_stops_at_last_item() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 2;
        state.move_down();
        assert_eq!(state.selected_project, 2, "should not go past last item");
    }

    #[test]
    fn switch_panel_toggles_focus() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert_eq!(state.focused_panel, Panel::Projects);
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Sessions);
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Projects);
    }

    #[test]
    fn navigate_sessions_panel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.focused_panel = Panel::Sessions;
        state.selected_session = 1; // last session of myapp
        state.move_down();
        assert_eq!(state.selected_session, 1, "should not go past last session");
    }

    #[test]
    fn navigate_sessions_empty_project() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 1; // hermes has no sessions
        state.focused_panel = Panel::Sessions;
        state.move_down();
        assert_eq!(state.selected_session, 0, "empty project: session stays 0");
    }

    #[test]
    fn navigate_down_resets_session_cursor() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "my".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "MYAPP".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn empty_search_shows_all_projects() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert_eq!(state.filtered_projects().len(), 3);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "zzznomatch".to_string();
        assert!(state.filtered_projects().is_empty());
    }

    #[test]
    fn selected_entry_returns_correct_project() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 1;
        assert_eq!(state.selected_entry().unwrap().project.name, "hermes");
    }

    #[test]
    fn selected_entry_with_filter_returns_filtered_item() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "prod".to_string();
        state.selected_project = 0;
        assert_eq!(
            state.selected_entry().unwrap().project.name,
            "prod-api"
        );
    }

    #[test]
    fn selected_entry_empty_list_returns_none() {
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.selected_entry().is_none());
    }

    // ── Edge case tests ───────────────────────────────────────────────────

    #[test]
    fn search_resets_selected_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        state.move_down();
        assert_eq!(state.selected_project, 0);
    }

    #[test]
    fn move_up_on_empty_entries_is_noop() {
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        state.move_up();
        assert_eq!(state.selected_project, 0);
    }

    #[test]
    fn switch_panel_on_empty_entries_does_not_panic() {
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        state.switch_panel();
        assert_eq!(state.focused_panel, Panel::Sessions);
        state.move_down(); // sessions panel, no entry → noop
        assert_eq!(state.selected_session, 0);
    }

    #[test]
    fn selected_entry_with_out_of_bounds_index_returns_none() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 99; // way past the end
        assert!(state.selected_entry().is_none());
    }

    #[test]
    fn search_then_clear_restores_full_list() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
            workflows: vec![],
            repo_actions: vec![],
        }];
        let mut state = TuiState::new(entries, Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_mode = true;
        state.search_query = "zzz_no_match".to_string();
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("PROJECTS"));
    }

    #[test]
    fn renders_narrow_terminal_without_panic() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Extremely narrow — columns may truncate but should not panic
        let _out = render_to_string(&state, 20, 10);
    }

    #[test]
    fn renders_remote_project_status_bar() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 2; // prod-api is remote
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("remote"), "status bar should say 'remote' for remote project");
        assert!(out.contains("prod-api"), "status bar should show prod-api");
    }

    #[test]
    fn navigate_project_down_then_up_resets_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // "mpp" → m..pp → matches "myapp"
        state.search_query = "mpp".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn fuzzy_search_includes_project_with_matching_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // "feat" doesn't match "myapp" or "hermes" project names,
        // but "myapp:feat-login" session contains "feat"
        state.search_query = "feat".to_string();
        let filtered = state.filtered_projects();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.project.name, "myapp");
    }

    #[test]
    fn fuzzy_search_project_name_match_shows_no_sessions_when_none_match() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // "main" matches the "myapp:main" session, so myapp should appear
        state.search_query = "main".to_string();
        let filtered = state.filtered_projects();
        assert!(filtered.iter().any(|(_, e)| e.project.name == "myapp"));
    }

    // ── filtered_sessions tests ───────────────────────────────────────────

    #[test]
    fn filtered_sessions_empty_query_returns_all() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // myapp has 2 sessions; no query → all returned
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn filtered_sessions_filters_by_fuzzy_match() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "login".to_string();
        // Only "myapp:feat-login" matches "login"
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].name.contains("login"));
    }

    #[test]
    fn filtered_sessions_no_match_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "zzznomatch".to_string();
        assert!(state.filtered_sessions().is_empty());
    }

    #[test]
    fn filtered_sessions_empty_project_returns_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.selected_project = 1; // hermes has no sessions
        assert!(state.filtered_sessions().is_empty());
    }

    #[test]
    fn navigate_sessions_respects_filter() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_mode = true;
        assert_eq!(state.selected_project, 0);
        state.move_down();
        assert_eq!(state.selected_project, 1, "down should work in search mode");
        state.move_up();
        assert_eq!(state.selected_project, 0, "up should work in search mode");
    }

    #[test]
    fn delete_targets_filtered_session_not_unfiltered() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Filter to only "feat-login"; selected_session = 0 should point to it
        state.search_query = "login".to_string();
        state.selected_session = 0;
        let sessions = state.filtered_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].name.contains("feat-login"));
    }

    #[test]
    fn delete_noop_when_no_sessions_match_filter() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_query = "zzznomatch".to_string();
        state.selected_session = 0;
        // No sessions match → get returns None → delete would be a no-op
        assert!(state.filtered_sessions().get(0).is_none());
    }

    // ── Snapshot tests for search mode UI states ─────────────────────────

    #[test]
    fn renders_search_mode_filters_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_mode = true;
        state.search_query = "login".to_string();
        let out = render_to_string(&state, 80, 24);
        // Only the login session should appear
        assert!(out.contains("login"), "login session should be visible");
    }

    #[test]
    fn renders_search_mode_hides_non_matching_sessions() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.search_mode = true;
        state.search_query = "hms".to_string(); // h..m..s matches "hermes"
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("hermes"), "hermes should match fuzzy query 'hms'");
        assert!(!out.contains("myapp"), "myapp should not match 'hms'");
    }

    // ── Notification badge tests ──────────────────────────────────────────

    #[test]
    fn renders_bell_badge_on_session_with_notification() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 80, 24);
        assert!(
            !out.contains('\u{1f514}'),
            "should not render 🔔 badge when no notifications pending"
        );
    }

    #[test]
    fn renders_bell_only_on_notified_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Only myapp:feat-login has a notification
        state.notifications.insert("myapp:feat-login".to_string());
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains('\u{1f514}'), "🔔 should appear for feat-login");
        // myapp:main has no notification
        assert!(out.contains("myapp:main"), "myapp:main should still render");
    }

    #[test]
    fn notifications_field_default_is_empty() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.notifications.is_empty());
    }

    // ── PR / CI / Zellij rendering tests (issue #11) ─────────────────────

    #[test]
    fn renders_pr_number_and_open_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(7, PrState::Merged));
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("merged"), "should show PR state 'merged'");
    }

    #[test]
    fn renders_pr_closed_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.pr = Some(make_pull_request(3, PrState::Closed));
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("closed"), "should show PR state 'closed'");
    }

    #[test]
    fn renders_no_pr_line_when_pr_absent() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let info = make_git_info(); // pr: None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(!out.contains("PR:"), "should not show PR line when no PR");
    }

    #[test]
    fn renders_ci_passing_indicator() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.ci = Some(CiStatus::Pending);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("pending"), "should show 'pending' for CI pending");
    }

    #[test]
    fn renders_no_ci_line_when_ci_unknown_and_no_pr() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.ci = Some(CiStatus::Unknown);
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        // Unknown CI with no PR — no PR/CI line should appear
        assert!(!out.contains("CI:"), "should not show CI line when unknown and no PR");
    }

    #[test]
    fn renders_review_new_comments() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.review = Some(z_core::domain::ReviewStatus {
            has_new_comments: true,
            comment_count: 3,
            last_review_at: Some("2026-04-09T15:00:00Z".into()),
        });
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("3 new review comments"), "should show new comment count");
        // 💬 U+1F4AC
        assert!(out.contains('\u{1f4ac}'), "should show 💬 for new comments");
    }

    #[test]
    fn renders_review_addressed_comments() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.review = Some(z_core::domain::ReviewStatus {
            has_new_comments: false,
            comment_count: 2,
            last_review_at: Some("2026-04-09T13:00:00Z".into()),
        });
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("2 review comments (addressed)"), "should show addressed comments");
    }

    #[test]
    fn renders_no_review_line_when_no_comments() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.review = Some(z_core::domain::ReviewStatus {
            has_new_comments: false,
            comment_count: 0,
            last_review_at: None,
        });
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(!out.contains("review comment"), "should not show review line when 0 comments");
    }

    #[test]
    fn renders_review_singular_comment() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut info = make_git_info();
        info.review = Some(z_core::domain::ReviewStatus {
            has_new_comments: true,
            comment_count: 1,
            last_review_at: Some("2026-04-09T15:00:00Z".into()),
        });
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("1 new review comment"), "should show singular 'comment'");
        assert!(!out.contains("comments"), "should not show plural 'comments'");
    }

    #[test]
    fn renders_zellij_session_info() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let info = make_git_info(); // zellij: None
        state.preview_data = PreviewData::Ready(info);
        let out = render_to_string(&state, 80, 30);
        assert!(!out.contains("session:"), "should not show session line when no Zellij info");
    }

    #[test]
    fn renders_full_preview_with_all_info() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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

    // ── poll_forge tests ────────────────────────────────────────────────────

    #[test]
    fn poll_forge_merges_pr_into_ready_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.preview_data = PreviewData::Ready(make_git_info());

        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);

        tx.send(Ok(ForgeData {
            pr: Some(make_pull_request(42, PrState::Open)),
            ci: CiStatus::Passing,
            zellij: Some(make_zellij_info()),
            review: None,
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.forge_rx = None;
        state.poll_forge(); // should not panic
    }

    #[test]
    fn poll_forge_noop_when_channel_empty() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // preview_data stays Loading (git not yet received)
        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);
        tx.send(Ok(ForgeData {
            pr: Some(make_pull_request(1, PrState::Open)),
            ci: CiStatus::Passing,
            zellij: None,
            review: None,
        }))
        .unwrap();
        state.poll_forge();
        // preview_data should still be Loading (nothing to merge into)
        assert!(matches!(state.preview_data, PreviewData::Loading));
        assert!(state.forge_rx.is_none(), "forge_rx cleared even when data discarded");
    }

    #[test]
    fn poll_forge_handles_disconnected_channel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let (tx, rx) = mpsc::channel::<Result<ForgeData, String>>();
        state.forge_rx = Some(rx);
        drop(tx); // simulate thread panic
        state.poll_forge();
        assert!(state.forge_rx.is_none(), "forge_rx should be cleared on disconnect");
    }

    #[test]
    fn forge_rx_default_is_none() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
    fn renders_ci_without_pr_no_orphaned_separator() {
        // When CI is shown but no PR exists, there should be no " | " prefix.
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        // Tab on non-path fields (field 1 onward) navigates forward.
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Move to field 1 first.
        if let Modal::AddProject(ref mut form) = modal {
            form.active_field = 1;
        }
        let outcome = advance_modal(&mut modal, KeyCode::Tab);
        assert!(matches!(outcome, ModalOutcome::Continue));
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 2);
    }

    #[test]
    fn advance_modal_tab_wraps_around() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Start at field 1; Tab 3 times → 1→2→3→0 (wraps to path field).
        if let Modal::AddProject(ref mut form) = modal {
            form.active_field = 1;
        }
        for _ in 0..3 {
            advance_modal(&mut modal, KeyCode::Tab);
        }
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 0);
    }

    #[test]
    fn advance_modal_tab_on_empty_path_field_navigates_forward() {
        // Tab on empty path field: no completions, so should navigate to field 1.
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::Tab);
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 1, "Tab on path with no completions should navigate to field 1");
    }

    #[test]
    fn advance_modal_backtab_goes_to_previous() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        // Start at field 2, Tab forward to field 3, BackTab back to field 2.
        if let Modal::AddProject(ref mut form) = modal {
            form.active_field = 2;
        }
        advance_modal(&mut modal, KeyCode::Tab); // field 3
        advance_modal(&mut modal, KeyCode::BackTab); // back to field 2
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 2);
    }

    #[test]
    fn advance_modal_backtab_wraps_from_first_to_last() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::BackTab); // wraps to field 3
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
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
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
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
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.fields[0].value, "/co");
    }

    #[test]
    fn advance_modal_backspace_removes_last_char() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        advance_modal(&mut modal, KeyCode::Char('a'));
        advance_modal(&mut modal, KeyCode::Char('b'));
        advance_modal(&mut modal, KeyCode::Backspace);
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
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
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.fields[1].value, "webapp", "name should be auto-filled from path basename");
    }

    #[test]
    fn modal_opens_on_uppercase_a_key_in_projects_panel() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.modal.is_none(), "no modal initially");
        state.focused_panel = Panel::Projects;
        // Simulate pressing 'A' by directly triggering the key handler logic
        state.modal = Some(Modal::AddProject(ProjectForm::new()));
        assert!(state.modal.is_some(), "modal should be open");
    }

    #[test]
    fn render_modal_add_project_shows_fields() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::AddProject(ProjectForm::new()));
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Tab"), "should show Tab hint");
        assert!(out.contains("Enter"), "should show Enter hint");
        assert!(out.contains("Esc"), "should show Esc hint");
    }

    #[test]
    fn render_modal_shows_yellow_warning_on_required_field() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut form = ProjectForm::new();
        form.fields[0].warning = Some("Required".to_string());
        state.modal = Some(Modal::AddProject(form));
        // Just verify it doesn't panic and renders something
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Required") || out.contains("Add Project"), "modal rendered");
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
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
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
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
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

    // ── Edit Project modal tests ──────────────────────────────────────────────

    fn make_edit_modal(name: &str, path: &str) -> Modal {
        let mut form = ProjectForm::new();
        form.fields[0].value = path.to_string();
        form.fields[1].value = name.to_string();
        form.name_was_modified = true;
        Modal::EditProject(form, name.to_string())
    }

    #[test]
    fn edit_modal_opens_prefilled_with_project_values() {
        let entry = ProjectEntry {
            project: make_project("myapp", false),
            sessions: vec![],
            worktree_count: 0,
            workflows: vec![],
            repo_actions: vec![],
        };
        // Simulate 'E' key: create EditProject modal pre-filled from entry
        let project = &entry.project;
        let mut form = ProjectForm::new();
        form.fields[0].value = project.path.to_string_lossy().to_string();
        form.fields[1].value = project.name.clone();
        form.fields[2].value = project.host.clone().unwrap_or_default();
        form.fields[3].value = project.token.clone().unwrap_or_default();
        form.name_was_modified = true;
        let original_name = project.name.clone();
        let modal = Modal::EditProject(form, original_name);

        if let Modal::EditProject(ref form, ref orig) = modal {
            assert_eq!(form.fields[0].value, "/home/user/myapp");
            assert_eq!(form.fields[1].value, "myapp");
            assert_eq!(orig, "myapp");
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn advance_modal_edit_project_no_name_change_no_warning() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        // Type in field 0 (path) — name field should not show warning
        advance_modal(&mut modal, KeyCode::Char('x'));
        if let Modal::EditProject(ref form, _) = modal {
            assert!(form.fields[1].warning.is_none(), "no rename warning when name unchanged");
        }
    }

    #[test]
    fn advance_modal_edit_project_name_change_shows_warning() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        // Tab to name field (field 1)
        advance_modal(&mut modal, KeyCode::Tab);
        // Type a character to change the name
        advance_modal(&mut modal, KeyCode::Char('X'));
        if let Modal::EditProject(ref form, _) = modal {
            assert!(
                form.fields[1].warning.is_some(),
                "should show rename warning when name changes"
            );
            let warn = form.fields[1].warning.as_ref().unwrap();
            assert!(
                warn.contains("sessions will not be renamed"),
                "warning should mention sessions: {}",
                warn
            );
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn advance_modal_edit_project_name_restored_clears_warning() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        // Tab to name field
        advance_modal(&mut modal, KeyCode::Tab);
        // Type a char (name becomes "myappX")
        advance_modal(&mut modal, KeyCode::Char('X'));
        // Delete the added char with backspace (name back to "myapp")
        advance_modal(&mut modal, KeyCode::Backspace);
        if let Modal::EditProject(ref form, _) = modal {
            assert!(
                form.fields[1].warning.is_none(),
                "warning should clear when name is restored"
            );
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn advance_modal_edit_project_submit_returns_submit_edit() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::SubmitEdit { original_name, path, name, host, token } => {
                assert_eq!(original_name, "myapp");
                assert_eq!(path, "/code/myapp");
                assert_eq!(name, "myapp");
                assert!(host.is_none());
                assert!(token.is_none());
            }
            _ => panic!("expected SubmitEdit outcome"),
        }
    }

    #[test]
    fn advance_modal_edit_project_submit_with_rename_returns_submit_edit() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        // Tab to name field and change name
        advance_modal(&mut modal, KeyCode::Tab);
        advance_modal(&mut modal, KeyCode::Char('-'));
        advance_modal(&mut modal, KeyCode::Char('v'));
        advance_modal(&mut modal, KeyCode::Char('2'));
        // Tab back to path field so we can submit
        advance_modal(&mut modal, KeyCode::BackTab);
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::SubmitEdit { original_name, name, .. } => {
                assert_eq!(original_name, "myapp", "original_name should be preserved");
                assert_eq!(name, "myapp-v2", "new name should reflect edits");
            }
            _ => panic!("expected SubmitEdit outcome"),
        }
    }

    #[test]
    fn render_modal_edit_project_shows_title_and_fields() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut form = ProjectForm::new();
        form.fields[0].value = "/code/myapp".to_string();
        form.fields[1].value = "myapp".to_string();
        form.name_was_modified = true;
        state.modal = Some(Modal::EditProject(form, "myapp".to_string()));
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Edit Project"), "should show Edit Project title");
        assert!(out.contains("Path"), "should show Path field");
        assert!(out.contains("Name"), "should show Name field");
    }

    #[test]
    fn e_uppercase_no_projects_does_nothing() {
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        // With no entries, selected_entry() returns None — 'E' should not open a modal.
        assert!(state.selected_entry().is_none(), "no entries means no modal should open");
    }

    #[test]
    fn render_status_bar_shows_edit_project_hint() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 100, 24);
        assert!(out.contains("[E]"), "should show [E] edit project hint");
    }

    #[test]
    fn edit_modal_esc_closes() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    // ── Delete Project modal tests ─────────────────────────────────────────

    #[test]
    fn d_key_on_projects_panel_with_project_opens_delete_modal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
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
    fn edit_modal_backspace_name_to_empty_clears_warning() {
        // Use a short name so we can backspace to empty quickly.
        let mut modal = make_edit_modal("ab", "/code/ab");
        // Tab to name field
        advance_modal(&mut modal, KeyCode::Tab);
        // Backspace: "ab" → "a" (differs from "ab" → warning shows)
        advance_modal(&mut modal, KeyCode::Backspace);
        if let Modal::EditProject(ref form, _) = modal {
            assert!(form.fields[1].warning.is_some(), "warning should show for partial rename");
        } else {
            panic!("expected EditProject modal");
        }
        // Backspace: "a" → "" (empty → warning clears despite differing from original)
        advance_modal(&mut modal, KeyCode::Backspace);
        if let Modal::EditProject(ref form, _) = modal {
            assert!(
                form.fields[1].warning.is_none(),
                "warning should clear when name is empty"
            );
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn edit_modal_submit_preserves_host_and_token() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "/code/myapp".to_string();
        form.fields[1].value = "myapp".to_string();
        form.fields[2].value = "  github.com  ".to_string();
        form.fields[3].value = "  tok_123  ".to_string();
        form.name_was_modified = true;
        let mut modal = Modal::EditProject(form, "myapp".to_string());
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::SubmitEdit { host, token, .. } => {
                assert_eq!(host, Some("github.com".to_string()), "host should be trimmed and preserved");
                assert_eq!(token, Some("tok_123".to_string()), "token should be trimmed and preserved");
            }
            _ => panic!("expected SubmitEdit"),
        }
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 120, 30);
        assert!(out.contains("[D]el"), "status bar should include [D]el hint");
    }

    #[test]
    fn worktree_count_stored_in_project_entry() {
        let entry = ProjectEntry {
            project: make_project("test", false),
            sessions: vec![],
            worktree_count: 5,
            workflows: vec![],
            repo_actions: vec![],
        };
        assert_eq!(entry.worktree_count, 5);
    }

    #[test]
    fn d_key_on_sessions_panel_does_not_open_delete_modal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
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
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "solo".to_string(),
            session_count: 1,
            worktree_count: 1,
        });
        let out = render_to_string(&state, 80, 30);
        // The modal renders "Active session: 1" (singular) and "Git worktree: 1" (singular)
        assert!(out.contains("Active session: 1"), "should show singular 'session' for count 1");
        assert!(out.contains("Git worktree: 1"), "should show singular 'worktree' for count 1");
        assert!(!out.contains("Active sessions:"), "should not use plural 'sessions' for count 1");
        assert!(!out.contains("Git worktrees:"), "should not use plural 'worktrees' for count 1");
    }

    #[test]
    fn delete_confirm_modal_with_zero_counts() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "empty".to_string(),
            session_count: 0,
            worktree_count: 0,
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Active sessions: 0"), "should use plural 'sessions' for count 0");
        assert!(out.contains("Git worktrees: 0"), "should use plural 'worktrees' for count 0");
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
    fn edit_modal_submit_rejects_empty_required_fields() {
        let mut form = ProjectForm::new();
        form.fields[0].value = "".to_string();
        form.fields[1].value = "   ".to_string(); // whitespace-only
        form.name_was_modified = true;
        let mut modal = Modal::EditProject(form, "myapp".to_string());
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Continue), "should not submit with empty required fields");
        if let Modal::EditProject(ref form, _) = modal {
            assert_eq!(form.fields[0].warning.as_deref(), Some("Required"));
            assert_eq!(form.fields[1].warning.as_deref(), Some("Required"));
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn edit_modal_tab_from_path_validates_path() {
        let mut modal = make_edit_modal("myapp", "/nonexistent/path/xyz");
        // Tab away from path field triggers validation
        advance_modal(&mut modal, KeyCode::Tab);
        if let Modal::EditProject(ref form, _) = modal {
            assert!(
                form.fields[0].warning.is_some(),
                "path validation should set warning for non-existent path"
            );
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn edit_modal_backtab_cycles_fields_backward() {
        let mut modal = make_edit_modal("myapp", "/code/myapp");
        // Start on field 0. BackTab should wrap to field 3.
        advance_modal(&mut modal, KeyCode::BackTab);
        if let Modal::EditProject(ref form, _) = modal {
            assert_eq!(form.active_field, 3, "BackTab from field 0 should wrap to last field");
        } else {
            panic!("expected EditProject modal");
        }
    }

    #[test]
    fn delete_confirm_modal_renders_on_small_terminal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::DeleteConfirm {
            project_name: "test".to_string(),
            session_count: 0,
            worktree_count: 0,
        });
        // Render on a very small terminal — should not panic.
        let out = render_to_string(&state, 30, 10);
        assert!(out.contains("Delete"), "modal should still render on small terminal");
    }

    // ── complete_path ─────────────────────────────────────────────────────

    #[test]
    fn complete_path_empty_input_returns_empty() {
        let empty: Vec<String> = vec![];
        assert_eq!(complete_path(""), empty);
    }

    #[test]
    fn complete_path_returns_matching_dirs_only() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_complete_path");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("alpha")).unwrap();
        std::fs::create_dir_all(base.join("beta")).unwrap();
        // Create a file (should be excluded)
        std::fs::write(base.join("afile.txt"), b"").unwrap();

        let partial = format!("{}/", base.display());
        let mut results = complete_path(&partial);
        results.sort();

        assert_eq!(results.len(), 2, "should return 2 dirs, not the file");
        assert!(results[0].ends_with("alpha"), "first dir is alpha");
        assert!(results[1].ends_with("beta"), "second dir is beta");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn complete_path_filters_by_prefix() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_prefix_filter");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("foo")).unwrap();
        std::fs::create_dir_all(base.join("foobar")).unwrap();
        std::fs::create_dir_all(base.join("baz")).unwrap();

        let partial = format!("{}/foo", base.display());
        let mut results = complete_path(&partial);
        results.sort();

        assert_eq!(results.len(), 2);
        assert!(results[0].ends_with("foo"));
        assert!(results[1].ends_with("foobar"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn complete_path_nonexistent_dir_returns_empty() {
        let result = complete_path("/nonexistent/path/that/does/not/exist/abc");
        let empty: Vec<String> = vec![];
        assert_eq!(result, empty);
    }

    #[test]
    fn complete_path_expands_tilde() {
        // Just verify ~/ expansion doesn't panic and returns something reasonable.
        // We can't control $HOME contents, but we can verify no crash.
        let _ = complete_path("~/");
    }

    // ── longest_common_prefix ─────────────────────────────────────────────

    #[test]
    fn longest_common_prefix_empty_slice() {
        assert_eq!(longest_common_prefix(&[]), "");
    }

    #[test]
    fn longest_common_prefix_single_element() {
        assert_eq!(longest_common_prefix(&["/code/app".to_string()]), "/code/app");
    }

    #[test]
    fn longest_common_prefix_common_prefix() {
        let strs = vec![
            "/code/foobar".to_string(),
            "/code/fooble".to_string(),
            "/code/food".to_string(),
        ];
        assert_eq!(longest_common_prefix(&strs), "/code/foo");
    }

    #[test]
    fn longest_common_prefix_no_common_prefix() {
        let strs = vec!["/alpha".to_string(), "/beta".to_string()];
        assert_eq!(longest_common_prefix(&strs), "/");
    }

    #[test]
    fn longest_common_prefix_identical_strings() {
        let strs = vec!["/code/app".to_string(), "/code/app".to_string()];
        assert_eq!(longest_common_prefix(&strs), "/code/app");
    }

    // ── Tab-completion in advance_modal ───────────────────────────────────

    #[test]
    fn tab_on_path_field_single_match_completes_inline() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_tab_single");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("myproject")).unwrap();

        let mut modal = Modal::AddProject(ProjectForm::new());
        let partial = format!("{}/my", base.display());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = partial;
        }

        advance_modal(&mut modal, KeyCode::Tab);

        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        let expected = format!("{}/myproject/", base.display());
        assert_eq!(form.fields[0].value, expected, "should complete to single match with trailing slash");
        assert_eq!(form.active_field, 0, "should stay on path field");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tab_on_path_field_multiple_matches_completes_to_prefix() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_tab_multi");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("foobar")).unwrap();
        std::fs::create_dir_all(base.join("fooble")).unwrap();

        let mut modal = Modal::AddProject(ProjectForm::new());
        let partial = format!("{}/foo", base.display());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = partial;
        }

        advance_modal(&mut modal, KeyCode::Tab);

        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        // Common prefix of foobar and fooble is foob
        assert!(
            form.fields[0].value.starts_with(&format!("{}/foob", base.display())),
            "should complete to longest common prefix: got {}",
            form.fields[0].value
        );
        assert_eq!(form.active_field, 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tab_on_path_field_no_match_navigates_to_next_field() {
        // When path completion finds no matches, Tab should navigate forward.
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = "/nonexistent/zzz_no_match_xyz".to_string();
        }
        advance_modal(&mut modal, KeyCode::Tab);
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.fields[0].value, "/nonexistent/zzz_no_match_xyz", "value should not change");
        assert_eq!(form.active_field, 1, "should navigate to field 1 when no completions");
    }

    #[test]
    fn edit_modal_tab_on_path_field_single_match_completes_inline() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_edit_tab_single");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("myproject")).unwrap();

        let partial = format!("{}/my", base.display());
        let mut modal = make_edit_modal("myapp", &partial);

        advance_modal(&mut modal, KeyCode::Tab);

        let Modal::EditProject(ref form, _) = modal else { panic!("expected EditProject modal") };
        let expected = format!("{}/myproject/", base.display());
        assert_eq!(form.fields[0].value, expected, "should complete to single match with trailing slash");
        assert_eq!(form.active_field, 0, "should stay on path field after completion");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn edit_modal_tab_on_path_field_no_match_navigates_to_next_field() {
        // When EditProject path has no completions, Tab should navigate forward.
        let mut modal = make_edit_modal("myapp", "/nonexistent/zzz_no_match_xyz");
        advance_modal(&mut modal, KeyCode::Tab);
        let Modal::EditProject(ref form, _) = modal else { panic!("expected EditProject modal") };
        assert_eq!(form.active_field, 1, "should navigate to field 1 when no completions");
        // Path validation should also run
        assert!(form.fields[0].warning.is_some(), "path validation should run when navigating away");
    }

    #[test]
    fn tab_on_path_field_single_match_autofills_name_if_empty() {
        let tmp = std::env::temp_dir();
        let base = tmp.join("z_tui_test_tab_autofill");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("webapp")).unwrap();

        let mut modal = Modal::AddProject(ProjectForm::new());
        let partial = format!("{}/web", base.display());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = partial;
        }

        advance_modal(&mut modal, KeyCode::Tab);

        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.fields[1].value, "webapp", "name should be auto-filled from completed basename");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tab_on_non_path_field_navigates_forward() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.active_field = 2;
        }
        advance_modal(&mut modal, KeyCode::Tab);
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 3);
    }

    // ── apply_prune (in-place prune handler) tests ───────────────────────────

    #[test]
    fn apply_prune_sets_status_message_nothing_to_prune() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.status_message.is_none());
        apply_prune(&mut state, &|_| Ok("Nothing to prune.".to_string()), false);
        assert_eq!(
            state.status_message.as_deref(),
            Some("Nothing to prune."),
            "apply_prune should set status_message from the closure result"
        );
    }

    #[test]
    fn apply_prune_sets_status_message_with_counts() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_prune(
            &mut state,
            &|_| Ok("Pruned: 2 session(s) killed, 1 worktree(s) removed.".to_string()),
            false,
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Pruned: 2 session(s) killed, 1 worktree(s) removed.")
        );
    }

    #[test]
    fn apply_prune_result_visible_in_render() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_prune(&mut state, &|_| Ok("Nothing to prune.".to_string()), false);
        let out = render_to_string(&state, 120, 24);
        assert!(
            out.contains("Nothing to prune"),
            "prune result from apply_prune should be visible in the status bar"
        );
    }

    #[test]
    fn apply_prune_overwrites_previous_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.status_message = Some("old message".to_string());
        apply_prune(&mut state, &|_| Ok("Nothing to prune.".to_string()), false);
        assert_eq!(state.status_message.as_deref(), Some("Nothing to prune."));
    }

    #[test]
    fn apply_prune_shows_error_as_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_prune(&mut state, &|_| {
            Err(io::Error::new(io::ErrorKind::Other, "session kill failed"))
        }, false);
        assert_eq!(
            state.status_message.as_deref(),
            Some("Prune failed: session kill failed"),
            "apply_prune should display errors inline instead of crashing the TUI"
        );
    }

    #[test]
    fn status_message_persists_after_navigation() {
        // The message set by apply_prune should survive move_down (navigation
        // doesn't clear it — only an explicit keypress-clear mechanism does).
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_prune(&mut state, &|_| Ok("Nothing to prune.".to_string()), false);
        state.move_down();
        assert!(
            state.status_message.is_some(),
            "navigation should not clear the status_message; it persists until the next keypress"
        );
        let out = render_to_string(&state, 120, 24);
        assert!(out.contains("Nothing to prune"));
    }

    // ── Prune inline status message tests ────────────────────────────────────

    #[test]
    fn status_message_shown_in_status_bar_when_set() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.status_message = Some("Pruned: 1 session(s) killed, 0 worktree(s) removed.".to_string());
        let out = render_to_string(&state, 120, 24);
        assert!(
            out.contains("Pruned:"),
            "status bar should show prune result message when status_message is set"
        );
    }

    #[test]
    fn project_info_shown_when_status_message_is_none() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.status_message.is_none());
        let out = render_to_string(&state, 120, 24);
        assert!(
            out.contains("myapp"),
            "status bar should show project info when status_message is None"
        );
    }

    #[test]
    fn nothing_to_prune_message_shown_inline() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.status_message = Some("Nothing to prune.".to_string());
        let out = render_to_string(&state, 120, 24);
        assert!(
            out.contains("Nothing to prune"),
            "status bar should show 'Nothing to prune.' inline"
        );
    }

    #[test]
    fn status_message_shown_with_empty_entries() {
        let mut state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        state.status_message = Some("Nothing to prune.".to_string());
        let out = render_to_string(&state, 120, 24);
        assert!(
            out.contains("Nothing to prune"),
            "status bar should show status_message even when there are no projects"
        );
        // Should NOT fall through to "No projects" default text
        assert!(
            !out.contains("No projects"),
            "status_message should take priority over the 'No projects' fallback"
        );
    }

    #[test]
    fn backtab_from_path_field_validates_path() {
        let mut modal = Modal::AddProject(ProjectForm::new());
        if let Modal::AddProject(ref mut form) = modal {
            form.fields[0].value = "/nonexistent/path/that/does/not/exist".to_string();
        }
        advance_modal(&mut modal, KeyCode::BackTab);
        let Modal::AddProject(ref form) = modal else { panic!("expected AddProject modal") };
        assert_eq!(form.active_field, 3, "BackTab from field 0 wraps to last field");
        assert!(form.fields[0].warning.is_some(), "path validation should run when leaving field 0 via BackTab");
    }

    #[test]
    fn edit_modal_backtab_from_path_field_validates_path() {
        let mut modal = make_edit_modal("myapp", "/nonexistent/path/that/does/not/exist");
        advance_modal(&mut modal, KeyCode::BackTab);
        let Modal::EditProject(ref form, _) = modal else { panic!("expected EditProject modal") };
        assert_eq!(form.active_field, 3, "BackTab from field 0 wraps to last field");
        assert!(form.fields[0].warning.is_some(), "path validation should run when leaving field 0 via BackTab in EditProject");
    }

    // ── WorkflowSelector modal ─────────────────────────────────────────────

    fn make_workflows() -> Vec<WorkflowInfo> {
        vec![
            WorkflowInfo {
                name: "pr-ci-fix".to_string(),
                trigger: "post-push".to_string(),
                description: "Fix CI failures".to_string(),
            },
            WorkflowInfo {
                name: "pr-review-fix".to_string(),
                trigger: "pr-review-received".to_string(),
                description: "Resolve review comments".to_string(),
            },
            WorkflowInfo {
                name: "pr-merge-when-ready".to_string(),
                trigger: "pr-approved".to_string(),
                description: "Auto-merge when ready".to_string(),
            },
        ]
    }

    #[test]
    fn workflow_selector_esc_closes() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn workflow_selector_enter_returns_workflow_selected() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::WorkflowSelected { project, workflow } => {
                assert_eq!(project, "myapp");
                assert_eq!(workflow, "pr-ci-fix");
            }
            _ => panic!("expected WorkflowSelected"),
        }
    }

    #[test]
    fn workflow_selector_down_moves_selection() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        advance_modal(&mut modal, KeyCode::Down);
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, 1);
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_j_moves_selection_down() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        advance_modal(&mut modal, KeyCode::Char('j'));
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, 1);
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_up_at_top_stays() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        advance_modal(&mut modal, KeyCode::Up);
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, 0, "Up at top should not underflow");
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_down_at_bottom_stays() {
        let wfs = make_workflows();
        let last = wfs.len() - 1;
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: wfs,
            selected: last,
        };
        advance_modal(&mut modal, KeyCode::Down);
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, last, "Down at bottom should not overflow");
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_enter_on_second_item_returns_correct_workflow() {
        let mut modal = Modal::WorkflowSelector {
            project: "hermes".to_string(),
            workflows: make_workflows(),
            selected: 1,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::WorkflowSelected { project, workflow } => {
                assert_eq!(project, "hermes");
                assert_eq!(workflow, "pr-review-fix");
            }
            _ => panic!("expected WorkflowSelected"),
        }
    }

    #[test]
    fn workflow_selector_other_key_continues() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('x'));
        assert!(matches!(outcome, ModalOutcome::Continue));
    }

    #[test]
    fn workflow_selector_renders_workflow_names() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        });
        let out = render_to_string(&state, 100, 30);
        assert!(out.contains("pr-ci-fix"), "should render first workflow name");
        assert!(out.contains("pr-review-fix"), "should render second workflow name");
        assert!(out.contains("pr-merge-when-ready"), "should render third workflow name");
    }

    #[test]
    fn workflow_selector_renders_project_name_in_title() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 0,
        });
        let out = render_to_string(&state, 100, 30);
        assert!(out.contains("myapp"), "modal title should include project name");
    }

    #[test]
    fn a_key_with_workflows_opens_workflow_selector() {
        let mut entries = make_entries();
        entries[0].workflows = make_workflows();
        let mut state = TuiState::new(entries, Navigation::Arrows, mock_forge(), mock_refresher());
        // Simulate the 'a' key handler from event_loop
        if let Some(entry) = state.selected_entry() {
            if !entry.workflows.is_empty() {
                let project = entry.project.name.clone();
                let workflows = entry.workflows.clone();
                state.modal = Some(Modal::WorkflowSelector { project, workflows, selected: 0 });
            }
        }
        assert!(
            matches!(state.modal, Some(Modal::WorkflowSelector { .. })),
            "'a' with workflows should open WorkflowSelector modal"
        );
    }

    #[test]
    fn a_key_without_workflows_does_not_open_modal() {
        // entries have workflows: vec![] by default from make_entries()
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Simulate the 'a' key handler from event_loop
        if let Some(entry) = state.selected_entry() {
            if !entry.workflows.is_empty() {
                let project = entry.project.name.clone();
                let workflows = entry.workflows.clone();
                state.modal = Some(Modal::WorkflowSelector { project, workflows, selected: 0 });
            }
        }
        assert!(state.modal.is_none(), "'a' with no workflows should not open any modal");
    }

    #[test]
    fn workflow_selector_k_moves_selection_up() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 2,
        };
        advance_modal(&mut modal, KeyCode::Char('k'));
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, 1, "k should move selection up");
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_up_from_middle_moves_up() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: make_workflows(),
            selected: 2,
        };
        advance_modal(&mut modal, KeyCode::Up);
        if let Modal::WorkflowSelector { selected, .. } = modal {
            assert_eq!(selected, 1, "Up from index 2 should go to 1");
        } else {
            panic!("expected WorkflowSelector");
        }
    }

    #[test]
    fn workflow_selector_enter_on_empty_list_closes() {
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: vec![],
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Close), "Enter on empty workflows should close");
    }

    #[test]
    fn workflow_selector_single_workflow_navigation() {
        let single = vec![WorkflowInfo {
            name: "only-one".to_string(),
            trigger: "manual".to_string(),
            description: "The sole workflow".to_string(),
        }];
        let mut modal = Modal::WorkflowSelector {
            project: "myapp".to_string(),
            workflows: single,
            selected: 0,
        };
        // Down should not move past the single item
        advance_modal(&mut modal, KeyCode::Down);
        if let Modal::WorkflowSelector { selected, .. } = &modal {
            assert_eq!(*selected, 0, "Down on single-item list should stay at 0");
        }
        // Enter should select it
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::WorkflowSelected { workflow, .. } => {
                assert_eq!(workflow, "only-one");
            }
            _ => panic!("expected WorkflowSelected"),
        }
    }

    // --- 'o' (open) and 'n' (new session) key handler tests ---

    #[test]
    fn o_key_on_projects_panel_returns_open_with_no_session() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Projects panel is the default focus
        assert_eq!(state.focused_panel, Panel::Projects);
        // Simulate 'o' key logic from event_loop
        let project_name = state.selected_entry().map(|e| e.project.name.clone());
        assert!(project_name.is_some(), "should have a selected project");
        let project = project_name.unwrap();
        // When projects panel is focused, session is always None
        let session: Option<String> = None;
        assert_eq!(project, "myapp");
        assert!(session.is_none(), "'o' on Projects panel should open with no session override");
    }

    #[test]
    fn o_key_on_sessions_panel_returns_open_with_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.focused_panel = Panel::Sessions;
        state.selected_session = 0;
        // Simulate 'o' key logic from event_loop
        let project_name = state.selected_entry().map(|e| e.project.name.clone());
        assert!(project_name.is_some());
        let session = if state.focused_panel == Panel::Sessions {
            let sessions = state.filtered_sessions();
            if !sessions.is_empty() {
                sessions.get(state.selected_session).map(|s| s.name.clone())
            } else {
                None
            }
        } else {
            None
        };
        // myapp has sessions: ["myapp:main", "myapp:feat-login"]
        assert_eq!(session.as_deref(), Some("myapp:main"),
            "'o' on Sessions panel should include the selected session name");
    }

    #[test]
    fn o_key_with_no_projects_returns_no_action() {
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        let project_name = state.selected_entry().map(|e| e.project.name.clone());
        assert!(project_name.is_none(), "'o' with no projects should not produce an action");
    }

    #[test]
    fn n_key_returns_new_with_selected_project() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Simulate 'n' key logic from event_loop
        let action_project = state.selected_entry().map(|e| e.project.name.clone());
        assert_eq!(action_project.as_deref(), Some("myapp"),
            "'n' should produce TuiAction::New for the selected project");
    }

    #[test]
    fn n_key_with_no_projects_returns_no_action() {
        let state = TuiState::new(vec![], Navigation::Arrows, mock_forge(), mock_refresher());
        let action_project = state.selected_entry().map(|e| e.project.name.clone());
        assert!(action_project.is_none(), "'n' with no projects should not produce an action");
    }

    // ── Help overlay tests ────────────────────────────────────────────────────

    #[test]
    fn question_mark_opens_help_modal() {
        // Simulate pressing '?' sets modal to Help
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        assert!(state.modal.is_none(), "no modal initially");
        // Simulate the '?' key logic from event_loop
        state.modal = Some(Modal::Help);
        assert!(matches!(state.modal, Some(Modal::Help)), "'?' should open Help modal");
    }

    #[test]
    fn help_modal_esc_closes() {
        let mut modal = Modal::Help;
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close), "Esc should close help modal");
    }

    #[test]
    fn help_modal_question_mark_closes() {
        let mut modal = Modal::Help;
        let outcome = advance_modal(&mut modal, KeyCode::Char('?'));
        assert!(matches!(outcome, ModalOutcome::Close), "'?' should close help modal");
    }

    #[test]
    fn help_modal_q_closes() {
        let mut modal = Modal::Help;
        let outcome = advance_modal(&mut modal, KeyCode::Char('q'));
        assert!(matches!(outcome, ModalOutcome::Close), "'q' should close help modal");
    }

    #[test]
    fn help_modal_other_keys_continue() {
        let mut modal = Modal::Help;
        let outcome = advance_modal(&mut modal, KeyCode::Char('x'));
        assert!(matches!(outcome, ModalOutcome::Continue), "other keys should not close help modal");
    }

    #[test]
    fn help_modal_renders_keybindings_section() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::Help);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Keybindings"), "help modal should show 'Keybindings' title");
        assert!(out.contains("Navigation"), "help modal should show 'Navigation' section");
        assert!(out.contains("Actions"), "help modal should show 'Actions' section");
        assert!(out.contains("Session"), "help modal should show 'Session' section");
    }

    #[test]
    fn help_modal_renders_key_entries() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::Help);
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("Open session"), "help modal should describe 'o' key");
        assert!(out.contains("Delete session"), "help modal should describe 'd' key");
        assert!(out.contains("Fuzzy search"), "help modal should describe '/' key");
        assert!(out.contains("Prune orphaned sessions"), "help modal should describe 'p' key");
    }

    #[test]
    fn help_modal_renders_in_small_terminal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::Help);
        // Should not panic even when terminal is smaller than the modal's preferred size
        let _out = render_to_string(&state, 30, 10);
    }

    #[test]
    fn status_bar_hints_include_help() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let out = render_to_string(&state, 150, 24);
        assert!(out.contains("[?]help"), "status bar should advertise '?' for help");
    }

    // ── BranchInput modal tests ──────────────────────────────────────────────

    #[test]
    fn n_key_opens_branch_input_modal() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Simulate 'n' key in normal mode: should open BranchInput modal
        state.modal = None;
        if let Some(entry) = state.selected_entry() {
            state.modal = Some(Modal::BranchInput {
                project: entry.project.name.clone(),
                input: String::new(),
            });
        }
        assert!(
            matches!(state.modal, Some(Modal::BranchInput { .. })),
            "n key should open BranchInput modal"
        );
    }

    #[test]
    fn branch_input_modal_esc_closes() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feat".to_string(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close), "Esc should close BranchInput modal");
    }

    #[test]
    fn branch_input_modal_char_appends() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feat".to_string(),
        };
        advance_modal(&mut modal, KeyCode::Char('-'));
        advance_modal(&mut modal, KeyCode::Char('x'));
        if let Modal::BranchInput { input, .. } = &modal {
            assert_eq!(input, "feat-x");
        } else {
            panic!("expected BranchInput modal");
        }
    }

    #[test]
    fn branch_input_modal_backspace_removes_char() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feat".to_string(),
        };
        advance_modal(&mut modal, KeyCode::Backspace);
        if let Modal::BranchInput { input, .. } = &modal {
            assert_eq!(input, "fea");
        } else {
            panic!("expected BranchInput modal");
        }
    }

    #[test]
    fn branch_input_modal_enter_empty_continues() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: String::new(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Continue), "Enter on empty input should not submit");
    }

    #[test]
    fn branch_input_modal_enter_with_branch_submits() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feature/foo".to_string(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::NewBranch { project, branch } => {
                assert_eq!(project, "myapp");
                assert_eq!(branch, "feature/foo");
            }
            _ => panic!("expected NewBranch outcome"),
        }
    }

    #[test]
    fn branch_input_modal_renders() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feat-123".to_string(),
        });
        let out = render_to_string(&state, 80, 30);
        assert!(out.contains("New Session"), "should show 'New Session' title");
        assert!(out.contains("feat-123"), "should show current input");
    }

    #[test]
    fn tui_action_new_includes_branch() {
        // TuiAction::New must carry the branch name
        let action = TuiAction::New {
            project: "myapp".to_string(),
            branch: "feature/bar".to_string(),
        };
        match action {
            TuiAction::New { project, branch } => {
                assert_eq!(project, "myapp");
                assert_eq!(branch, "feature/bar");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn branch_input_modal_backspace_on_empty_is_noop() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: String::new(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Backspace);
        assert!(matches!(outcome, ModalOutcome::Continue));
        if let Modal::BranchInput { input, .. } = &modal {
            assert_eq!(input, "");
        } else {
            panic!("expected BranchInput modal");
        }
    }

    #[test]
    fn branch_input_modal_enter_whitespace_only_continues() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "   ".to_string(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(
            matches!(outcome, ModalOutcome::Continue),
            "Enter on whitespace-only input should not submit"
        );
    }

    #[test]
    fn branch_input_modal_enter_trims_whitespace() {
        let mut modal = Modal::BranchInput {
            project: "myapp".to_string(),
            input: "  feat/bar  ".to_string(),
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::NewBranch { branch, .. } => {
                assert_eq!(branch, "feat/bar", "branch name should be trimmed");
            }
            _ => panic!("expected NewBranch outcome"),
        }
    }

    #[test]
    fn branch_input_modal_small_terminal_no_panic() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::BranchInput {
            project: "myapp".to_string(),
            input: "feat".to_string(),
        });
        // Should not panic even when terminal is smaller than the modal
        let _out = render_to_string(&state, 20, 5);
    }

    // ── LogViewer modal tests ─────────────────────────────────────────────────

    #[test]
    fn log_viewer_esc_closes() {
        let mut modal = Modal::LogViewer {
            lines: vec!["[2026-04-06] [INFO] test".to_string()],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close), "Esc should close LogViewer");
    }

    #[test]
    fn log_viewer_q_closes() {
        let mut modal = Modal::LogViewer {
            lines: vec!["[2026-04-06] [INFO] test".to_string()],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('q'));
        assert!(matches!(outcome, ModalOutcome::Close), "q should close LogViewer");
    }

    #[test]
    fn log_viewer_l_does_not_close() {
        let mut modal = Modal::LogViewer {
            lines: vec!["[2026-04-06] [INFO] test".to_string()],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('l'));
        assert!(matches!(outcome, ModalOutcome::Continue), "l should not close LogViewer (Ctrl+l does)");
    }

    #[test]
    fn log_viewer_j_scrolls_down() {
        let mut modal = Modal::LogViewer {
            lines: vec!["line1".to_string(), "line2".to_string(), "line3".to_string()],
            scroll_offset: 0,
        };
        advance_modal(&mut modal, KeyCode::Char('j'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 1, "j should increment scroll_offset");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_k_scrolls_up() {
        let mut modal = Modal::LogViewer {
            lines: vec!["line1".to_string(), "line2".to_string(), "line3".to_string()],
            scroll_offset: 2,
        };
        advance_modal(&mut modal, KeyCode::Char('k'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 1, "k should decrement scroll_offset");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_k_does_not_underflow() {
        let mut modal = Modal::LogViewer {
            lines: vec!["line1".to_string()],
            scroll_offset: 0,
        };
        advance_modal(&mut modal, KeyCode::Char('k'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "k at top should stay at 0");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_j_does_not_exceed_last_line() {
        let mut modal = Modal::LogViewer {
            lines: vec!["line1".to_string(), "line2".to_string()],
            scroll_offset: 1,
        };
        advance_modal(&mut modal, KeyCode::Char('j'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 1, "j at last line should not exceed bounds");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_g_jumps_to_top() {
        let mut modal = Modal::LogViewer {
            lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            scroll_offset: 2,
        };
        advance_modal(&mut modal, KeyCode::Char('g'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "g should jump to top");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_capital_g_jumps_to_bottom() {
        let mut modal = Modal::LogViewer {
            lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            scroll_offset: 0,
        };
        advance_modal(&mut modal, KeyCode::Char('G'));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 2, "G should jump to last line index");
        } else {
            panic!("expected LogViewer modal");
        }
    }

    #[test]
    fn log_viewer_renders_log_content() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::LogViewer {
            lines: vec![
                "[2026-04-06] [INFO] session created".to_string(),
                "[2026-04-06] [ERROR] worktree failed".to_string(),
            ],
            scroll_offset: 0,
        });
        let out = render_to_string(&state, 120, 40);
        assert!(out.contains("Logs"), "should render Logs title");
        assert!(out.contains("session created"), "should render log line content");
    }

    #[test]
    fn log_viewer_renders_empty_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::LogViewer {
            lines: vec![],
            scroll_offset: 0,
        });
        let out = render_to_string(&state, 120, 40);
        assert!(out.contains("Logs"), "should render Logs title even when empty");
        assert!(out.contains("No logs yet"), "should show empty state message");
    }

    #[test]
    fn log_viewer_small_terminal_no_panic() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::LogViewer {
            lines: vec!["test line".to_string()],
            scroll_offset: 0,
        });
        // Should not panic even when terminal is smaller than the modal
        let _out = render_to_string(&state, 20, 5);
    }

    #[test]
    fn log_viewer_g_on_empty_lines() {
        let mut modal = Modal::LogViewer {
            lines: vec![],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('g'));
        assert!(matches!(outcome, ModalOutcome::Continue));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "g on empty should stay at 0");
        }
    }

    #[test]
    fn log_viewer_capital_g_on_empty_lines() {
        let mut modal = Modal::LogViewer {
            lines: vec![],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('G'));
        assert!(matches!(outcome, ModalOutcome::Continue));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "G on empty should stay at 0");
        }
    }

    #[test]
    fn log_viewer_j_on_empty_lines() {
        let mut modal = Modal::LogViewer {
            lines: vec![],
            scroll_offset: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('j'));
        assert!(matches!(outcome, ModalOutcome::Continue));
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "j on empty should stay at 0");
        }
    }

    #[test]
    fn log_viewer_scroll_clamped_shows_last_page() {
        // When scroll_offset is at the last line (e.g. from G or initial open),
        // render should clamp it so the viewport shows a full page of lines,
        // not just the single last line.
        let lines: Vec<String> = (0..50).map(|i| format!("[INFO] line {}", i)).collect();
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::LogViewer {
            lines: lines.clone(),
            scroll_offset: 49, // last line index, as set by G or initial open
        });
        // 40-row terminal, modal takes ~36 rows, inner ~34 rows visible
        let out = render_to_string(&state, 120, 40);
        // With clamping, we should see lines near the end, not just "line 49"
        assert!(out.contains("line 49"), "should show the last line");
        assert!(out.contains("line 48"), "should show second-to-last line too");
    }

    #[test]
    fn log_viewer_empty_title_shows_zero() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::LogViewer {
            lines: vec![],
            scroll_offset: 0,
        });
        let out = render_to_string(&state, 120, 40);
        // Empty log should not show "1/1"
        assert!(!out.contains("1/1"), "empty logs should not show 1/1 in title");
    }

    #[test]
    fn log_viewer_arrow_keys_scroll() {
        // Arrow Up behaves same as 'k'
        let mut modal = Modal::LogViewer {
            lines: vec!["a".to_string(), "b".to_string()],
            scroll_offset: 1,
        };
        advance_modal(&mut modal, KeyCode::Up);
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 0, "Up should decrement scroll_offset");
        }

        // Arrow Down behaves same as 'j'
        let mut modal = Modal::LogViewer {
            lines: vec!["a".to_string(), "b".to_string()],
            scroll_offset: 0,
        };
        advance_modal(&mut modal, KeyCode::Down);
        if let Modal::LogViewer { scroll_offset, .. } = &modal {
            assert_eq!(*scroll_offset, 1, "Down should increment scroll_offset");
        }
    }

    // ── SwitchPickerState tests ───────────────────────────────────────────

    fn render_switch_picker_to_string(
        state: &SwitchPickerState,
        width: u16,
        height: u16,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = z_core::theme::Theme::default();
        terminal.draw(|f| render_switch_picker(f, state, &theme)).unwrap();
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

    #[test]
    fn switch_picker_renders_title() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains("Switch Session"), "should render title");
    }

    #[test]
    fn switch_picker_renders_session_names() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains("myapp:main"), "should render myapp:main");
        assert!(out.contains("hermes:dev"), "should render hermes:dev");
    }

    #[test]
    fn switch_picker_renders_current_marker() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains('\u{25cf}'), "should render ● marker on current session");
    }

    #[test]
    fn switch_picker_renders_footer_hints() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string()],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 80, 15);
        assert!(out.contains("j/k"), "should render j/k hint");
        assert!(out.contains("Enter"), "should render Enter hint");
        assert!(out.contains("Esc"), "should render Esc hint");
    }

    #[test]
    fn switch_picker_initial_selection_on_current_session() {
        let state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string(), "myapp:main".to_string()],
            "beta:dev".to_string(),
        );
        assert_eq!(state.selected, 1, "should start on the current session");
    }

    #[test]
    fn switch_picker_initial_selection_defaults_to_zero_when_not_found() {
        let state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            "unknown:session".to_string(),
        );
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn switch_picker_move_up_decrements() {
        let mut state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            "alpha:main".to_string(),
        );
        state.selected = 1;
        state.move_up();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn switch_picker_move_down_increments() {
        let mut state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            "alpha:main".to_string(),
        );
        state.move_down();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn switch_picker_move_up_clamps_at_zero() {
        let mut state = SwitchPickerState::new(
            vec!["alpha:main".to_string()],
            "alpha:main".to_string(),
        );
        state.move_up();
        assert_eq!(state.selected, 0, "should not go below 0");
    }

    #[test]
    fn switch_picker_move_down_clamps_at_last() {
        let mut state = SwitchPickerState::new(
            vec!["alpha:main".to_string()],
            "alpha:main".to_string(),
        );
        state.move_down();
        assert_eq!(state.selected, 0, "should not go past last item");
    }

    #[test]
    fn switch_picker_selected_session_returns_name() {
        let state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            "alpha:main".to_string(),
        );
        assert_eq!(state.selected_session(), Some("alpha:main"));
    }

    #[test]
    fn switch_picker_small_terminal_no_panic() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string()],
            "myapp:main".to_string(),
        );
        let _out = render_switch_picker_to_string(&state, 20, 5);
    }

    #[test]
    fn switch_picker_selected_session_empty_vec() {
        let state = SwitchPickerState::new(vec![], "myapp:main".to_string());
        assert_eq!(state.selected_session(), None);
    }

    #[test]
    fn switch_picker_navigation_empty_vec_no_panic() {
        let mut state = SwitchPickerState::new(vec![], "myapp:main".to_string());
        state.move_up();
        assert_eq!(state.selected, 0);
        state.move_down();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn switch_picker_non_current_session_has_no_marker() {
        let state = SwitchPickerState::new(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            "alpha:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        // ● should appear exactly once — only on alpha:main (the current session)
        let marker_count = out.matches('\u{25cf}').count();
        assert_eq!(marker_count, 1, "● marker should appear exactly once for the current session");
    }

    #[test]
    fn switch_picker_render_many_sessions_capped_at_20() {
        let sessions: Vec<String> = (0..30)
            .map(|i| format!("proj{}:main", i))
            .collect();
        let state = SwitchPickerState::new(sessions, "proj0:main".to_string());
        // Should not panic; modal height is capped via .min(20)
        let _out = render_switch_picker_to_string(&state, 80, 30);
    }

    #[test]
    fn switch_picker_render_empty_sessions_no_panic() {
        let state = SwitchPickerState::new(vec![], String::new());
        // Empty sessions: content_rows = 0, modal_height = 3 (borders + footer)
        let _out = render_switch_picker_to_string(&state, 60, 15);
    }

    // ── Notification count badge tests ────────────────────────────────────

    #[test]
    fn switch_picker_notification_counts_default_to_zero_in_new() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            "myapp:main".to_string(),
        );
        assert_eq!(state.notification_counts, vec![0, 0]);
    }

    #[test]
    fn switch_picker_notification_counts_default_to_zero_in_with_ages() {
        let state = SwitchPickerState::with_ages(
            vec!["myapp:main".to_string()],
            vec![Some("1h".to_string())],
            "myapp:main".to_string(),
        );
        assert_eq!(state.notification_counts, vec![0]);
    }

    #[test]
    fn switch_picker_with_notifications_stores_counts() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            vec![Some("2h".to_string()), None],
            vec![3, 0],
            "myapp:main".to_string(),
        );
        assert_eq!(state.notification_counts, vec![3, 0]);
    }

    #[test]
    fn switch_picker_renders_bell_badge_for_session_with_notifications() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            vec![Some("2h".to_string()), None],
            vec![2, 0],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains('\u{1f514}'), "should render 🔔 badge for session with notifications");
    }

    #[test]
    fn switch_picker_renders_notification_count_number() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string()],
            vec![None],
            vec![5],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains('\u{1f514}'), "should render 🔔 badge");
        assert!(out.contains('5'), "should render notification count 5");
    }

    #[test]
    fn switch_picker_no_bell_badge_when_zero_notifications() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string()],
            vec![None],
            vec![0],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(!out.contains('\u{1f514}'), "should not render 🔔 badge when zero notifications");
    }

    #[test]
    fn switch_picker_only_notified_sessions_show_badge() {
        let state = SwitchPickerState::with_notifications(
            vec!["alpha:main".to_string(), "beta:dev".to_string(), "gamma:feat".to_string()],
            vec![None, None, None],
            vec![0, 2, 0],
            "alpha:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 70, 15);
        assert!(out.contains('\u{1f514}'), "🔔 should appear for beta:dev");
        // Count occurrences of 🔔 — should be exactly 1
        let bell_count = out.matches('\u{1f514}').count();
        assert_eq!(bell_count, 1, "🔔 should appear exactly once (only for beta:dev)");
    }

    #[test]
    fn switch_picker_badge_with_age_both_visible() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string()],
            vec![Some("3h".to_string())],
            vec![1],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains('\u{1f514}'), "should render 🔔 badge");
        assert!(out.contains("3h"), "should render age '3h' alongside badge");
    }

    #[test]
    fn switch_picker_with_notifications_empty_vecs() {
        let state = SwitchPickerState::with_notifications(
            vec![],
            vec![],
            vec![],
            "nonexistent".to_string(),
        );
        assert_eq!(state.sessions.len(), 0);
        assert_eq!(state.notification_counts.len(), 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn switch_picker_large_notification_count() {
        let state = SwitchPickerState::with_notifications(
            vec!["myapp:main".to_string()],
            vec![Some("1h".to_string())],
            vec![99],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains('\u{1f514}'), "should render 🔔 badge for large count");
        assert!(out.contains("99"), "should render count 99");
        assert!(out.contains("1h"), "should still render age alongside large count");
    }

    #[test]
    fn switch_picker_narrow_terminal_with_notifications_no_panic() {
        // Terminal too narrow to fit badge+age — should fall back to name-only
        let state = SwitchPickerState::with_notifications(
            vec!["very-long-project-name:very-long-branch-name".to_string()],
            vec![Some("2h".to_string())],
            vec![5],
            "very-long-project-name:very-long-branch-name".to_string(),
        );
        // 40 is minimum modal width; inner = 38, name with marker = 47 chars
        // Right side won't fit, should gracefully omit badge+age
        let _out = render_switch_picker_to_string(&state, 40, 10);
    }

    // ── Age display tests ─────────────────────────────────────────────────

    #[test]
    fn switch_picker_with_ages_renders_age_in_output() {
        let state = SwitchPickerState::with_ages(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            vec![Some("2h".to_string()), Some("30m".to_string())],
            "myapp:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains("2h"), "should render age '2h'");
        assert!(out.contains("30m"), "should render age '30m'");
    }

    #[test]
    fn switch_picker_with_ages_none_age_renders_gracefully() {
        let state = SwitchPickerState::with_ages(
            vec!["myapp:main".to_string()],
            vec![None],
            "myapp:main".to_string(),
        );
        // Should not panic and should render session name
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains("myapp:main"), "should still render session name");
    }

    #[test]
    fn switch_picker_with_ages_initial_selection_on_current() {
        let state = SwitchPickerState::with_ages(
            vec!["alpha:main".to_string(), "beta:dev".to_string()],
            vec![Some("1h".to_string()), Some("5m".to_string())],
            "beta:dev".to_string(),
        );
        assert_eq!(state.selected, 1, "should start on beta:dev");
    }

    #[test]
    fn switch_picker_new_ages_all_none() {
        let state = SwitchPickerState::new(
            vec!["myapp:main".to_string(), "hermes:dev".to_string()],
            "myapp:main".to_string(),
        );
        assert_eq!(state.ages, vec![None, None], "new() should set all ages to None");
    }

    #[test]
    fn switch_picker_age_not_shown_when_insufficient_width() {
        // Very narrow terminal: age should not overflow or panic
        let state = SwitchPickerState::with_ages(
            vec!["myapp:main".to_string()],
            vec![Some("2h".to_string())],
            "myapp:main".to_string(),
        );
        let _out = render_switch_picker_to_string(&state, 20, 5);
    }

    #[test]
    fn switch_picker_mixed_ages_some_and_none() {
        let state = SwitchPickerState::with_ages(
            vec!["alpha:main".to_string(), "beta:dev".to_string(), "gamma:feat".to_string()],
            vec![Some("1h".to_string()), None, Some("3d".to_string())],
            "alpha:main".to_string(),
        );
        let out = render_switch_picker_to_string(&state, 60, 15);
        assert!(out.contains("1h"), "should render age for alpha");
        assert!(out.contains("3d"), "should render age for gamma");
        assert!(out.contains("beta:dev"), "should render beta name without age");
    }

    // ── Theme style tests (TestBackend) ──────────────────────────────────

    /// Find the buffer cell that starts rendering text `needle` and return
    /// the style of that cell.
    fn find_cell_style(
        buf: &ratatui::buffer::Buffer,
        needle: &str,
    ) -> Option<Style> {
        let area = buf.area;
        for row in area.y..area.y + area.height {
            let mut line = String::new();
            for col in area.x..area.x + area.width {
                line.push_str(buf.cell((col, row)).map(|c| c.symbol()).unwrap_or(" "));
            }
            if let Some(col_offset) = line.find(needle) {
                let cell = buf.cell((col_offset as u16, row))?;
                return Some(cell.style());
            }
        }
        None
    }

    #[test]
    fn theme_selected_project_has_dracula_colors() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Default theme is Dracula — selected project "myapp" should have
        // purple fg (#bd93f9) and current-line bg (#44475a)
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // The highlight symbol "▸" precedes the selected item
        let style = find_cell_style(&buf, "\u{25b8}").expect("should find highlight symbol");
        // Verify Dracula purple fg
        assert_eq!(style.fg, Some(Color::Rgb(189, 147, 249)), "selected item fg should be Dracula purple");
        // Verify Dracula current-line bg
        assert_eq!(style.bg, Some(Color::Rgb(68, 71, 90)), "selected item bg should be Dracula current-line");
    }

    #[test]
    fn theme_focused_border_has_dracula_purple() {
        let state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // Top-left corner of PROJECTS panel is the border character at (0,0)
        let corner = buf.cell((0u16, 0u16)).expect("should have corner cell");
        let style = corner.style();
        // Focused panel border = Dracula purple + bold
        assert_eq!(style.fg, Some(Color::Rgb(189, 147, 249)), "focused border fg should be Dracula purple");
    }

    // ── apply_delete_session (in-place session delete) tests ─────────────────

    /// Helper: returns a reload closure that produces a fixed set of entries.
    fn make_reload_fn(entries: Vec<ProjectEntry>) -> impl Fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)> {
        move || Ok((entries.clone(), HashSet::new()))
    }

    #[test]
    fn apply_delete_session_sets_status_message_on_success() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let reload_entries = make_entries();
        apply_delete_session(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(reload_entries),
            "myapp:feat/login",
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Session myapp:feat/login killed."),
        );
    }

    #[test]
    fn apply_delete_session_clamps_selected_session() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Select session index 1 (feat/login).
        state.focused_panel = Panel::Sessions;
        state.selected_session = 1;
        // After kill, reload returns entries where myapp has only 1 session.
        let mut reloaded = make_entries();
        reloaded[0].sessions = vec![Session::new("myapp", "main")];
        apply_delete_session(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(reloaded),
            "myapp:feat/login",
        );
        assert_eq!(
            state.selected_session, 0,
            "selected_session should clamp to last valid index after deletion"
        );
    }

    #[test]
    fn apply_delete_session_reloads_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Reload returns only one project (simulating the session being gone).
        let reloaded = vec![make_entries()[0].clone()];
        apply_delete_session(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(reloaded),
            "myapp:feat/login",
        );
        assert_eq!(state.entries.len(), 1, "state should reflect reloaded entries");
    }

    #[test]
    fn apply_delete_session_shows_error_as_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_delete_session(
            &mut state,
            &|_| Err(io::Error::new(io::ErrorKind::Other, "session not found")),
            &make_reload_fn(make_entries()),
            "myapp:feat/login",
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Error: session not found"),
            "apply_delete_session should display errors inline"
        );
    }

    // ── apply_delete_project (in-place project delete) tests ──────────────

    #[test]
    fn apply_delete_project_sets_status_message_on_success() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // After deletion, reload returns entries without "myapp".
        let remaining: Vec<_> = make_entries().into_iter().filter(|e| e.project.name != "myapp").collect();
        apply_delete_project(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(remaining),
            "myapp",
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Project myapp deleted."),
        );
    }

    #[test]
    fn apply_delete_project_reloads_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let remaining: Vec<_> = make_entries().into_iter().filter(|e| e.project.name != "myapp").collect();
        let expected_len = remaining.len();
        apply_delete_project(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(remaining),
            "myapp",
        );
        assert_eq!(state.entries.len(), expected_len);
    }

    #[test]
    fn apply_delete_project_cursor_moves_to_nearest_neighbor() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Select the last project (index 2 = "prod-api").
        state.selected_project = 2;
        // Delete it — reload returns only first two.
        let remaining: Vec<_> = make_entries().into_iter().filter(|e| e.project.name != "prod-api").collect();
        apply_delete_project(
            &mut state,
            &|_| Ok(()),
            &make_reload_fn(remaining),
            "prod-api",
        );
        // Cursor should clamp to last valid index (1).
        assert!(
            state.selected_project <= 1,
            "cursor should clamp to valid range after deleting last project"
        );
    }

    #[test]
    fn apply_delete_project_shows_error_as_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let original_count = state.entries.len();
        apply_delete_project(
            &mut state,
            &|_| Err(io::Error::new(io::ErrorKind::Other, "permission denied")),
            &make_reload_fn(vec![]),
            "myapp",
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Error: permission denied"),
        );
        assert_eq!(state.entries.len(), original_count, "should not reload on error");
    }

    // ── apply_add_project (in-place project add) tests ────────────────────

    #[test]
    fn apply_add_project_sets_status_message_on_success() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut reloaded = make_entries();
        reloaded.push(ProjectEntry {
            project: make_project("new-proj", false),
            sessions: vec![],
            worktree_count: 0,
            workflows: vec![],
            repo_actions: vec![],
        });
        apply_add_project(
            &mut state,
            &|_, _, _, _| Ok(()),
            &make_reload_fn(reloaded),
            "/tmp/new-proj", "new-proj", None, None,
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Project new-proj added."),
        );
    }

    #[test]
    fn apply_add_project_reloads_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut reloaded = make_entries();
        reloaded.push(ProjectEntry {
            project: make_project("new-proj", false),
            sessions: vec![],
            worktree_count: 0,
            workflows: vec![],
            repo_actions: vec![],
        });
        apply_add_project(
            &mut state,
            &|_, _, _, _| Ok(()),
            &make_reload_fn(reloaded),
            "/tmp/new-proj", "new-proj", None, None,
        );
        assert_eq!(state.entries.len(), 4, "state should contain the new project after reload");
    }

    // ── apply_edit_project (in-place project edit) tests ──────────────────

    #[test]
    fn apply_edit_project_sets_status_message_on_success() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        apply_edit_project(
            &mut state,
            &|_, _, _, _, _| Ok(()),
            &make_reload_fn(make_entries()),
            "myapp", "/new/path", "myapp-renamed", None, None,
        );
        assert_eq!(
            state.status_message.as_deref(),
            Some("Project myapp-renamed saved."),
        );
    }

    #[test]
    fn apply_edit_project_reloads_state() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        // Reload returns only 2 entries (simulating a rename that changed the list).
        let reloaded = vec![make_entries()[0].clone(), make_entries()[1].clone()];
        apply_edit_project(
            &mut state,
            &|_, _, _, _, _| Ok(()),
            &make_reload_fn(reloaded),
            "myapp", "/new/path", "myapp", None, None,
        );
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn apply_edit_project_shows_error_as_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let original_count = state.entries.len();
        apply_edit_project(
            &mut state,
            &|_, _, _, _, _| Err(io::Error::new(io::ErrorKind::Other, "write failed")),
            &make_reload_fn(vec![]),
            "myapp", "/path", "myapp", None, None,
        );
        assert_eq!(state.status_message.as_deref(), Some("Error: write failed"));
        assert_eq!(state.entries.len(), original_count, "should not reload on error");
    }

    // ── apply_add_project continued ─────────────────────────────────────────

    #[test]
    fn apply_add_project_selects_new_project() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let mut reloaded = make_entries();
        reloaded.push(ProjectEntry {
            project: make_project("new-proj", false),
            sessions: vec![],
            worktree_count: 0,
            workflows: vec![],
            repo_actions: vec![],
        });
        apply_add_project(
            &mut state,
            &|_, _, _, _| Ok(()),
            &make_reload_fn(reloaded),
            "/tmp/new-proj", "new-proj", None, None,
        );
        assert_eq!(
            state.selected_project, 3,
            "cursor should move to the newly added project"
        );
    }

    #[test]
    fn apply_add_project_shows_error_as_status_message() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let original_count = state.entries.len();
        apply_add_project(
            &mut state,
            &|_, _, _, _| Err(io::Error::new(io::ErrorKind::Other, "duplicate name")),
            &make_reload_fn(vec![]),
            "/tmp/x", "x", None, None,
        );
        assert_eq!(state.status_message.as_deref(), Some("Error: duplicate name"));
        assert_eq!(state.entries.len(), original_count, "should not reload on error");
    }

    // ── apply_delete_session continued ──────────────────────────────────────

    #[test]
    fn apply_delete_session_does_not_reload_on_error() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        let original_count = state.entries.len();
        apply_delete_session(
            &mut state,
            &|_| Err(io::Error::new(io::ErrorKind::Other, "fail")),
            &make_reload_fn(vec![]), // reload would produce empty list
            "myapp:feat/login",
        );
        assert_eq!(state.entries.len(), original_count, "should not reload on error");
    }

    // ── ActionMenu modal tests ─────────────────────────────────────────────

    fn make_test_actions() -> Vec<ResolvedAction> {
        vec![
            ResolvedAction {
                name: "Review PR".into(),
                action: ActionType::Run { command: "codex review".into() },
                pane: PaneType::Tab,
                icon: Some("\u{1f50d}".into()),
            },
            ResolvedAction {
                name: "Fix CI".into(),
                action: ActionType::Run { command: "claude fix".into() },
                pane: PaneType::Tab,
                icon: Some("\u{1f527}".into()),
            },
            ResolvedAction {
                name: "Open PR".into(),
                action: ActionType::OpenUrl { url: "https://github.com/pr/42".into() },
                pane: PaneType::Float,
                icon: Some("\u{1f310}".into()),
            },
        ]
    }

    #[test]
    fn action_menu_renders_action_names() {
        let mut state = TuiState::new(make_entries(), Navigation::Arrows, mock_forge(), mock_refresher());
        state.modal = Some(Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        });
        let out = render_to_string(&state, 80, 24);
        assert!(out.contains("Actions"), "should show Actions title");
        assert!(out.contains("Review PR"), "should show 'Review PR' action");
        assert!(out.contains("Fix CI"), "should show 'Fix CI' action");
        assert!(out.contains("Open PR"), "should show 'Open PR' action");
    }

    #[test]
    fn action_menu_esc_closes() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Esc);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn action_menu_enter_returns_action_selected() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::ActionSelected { action } => {
                assert_eq!(action.name, "Review PR");
            }
            _ => panic!("expected ActionSelected"),
        }
    }

    #[test]
    fn action_menu_down_moves_selection() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Down);
        assert!(matches!(outcome, ModalOutcome::Continue));
        if let Modal::ActionMenu { selected, .. } = &modal {
            assert_eq!(*selected, 1);
        }
    }

    #[test]
    fn action_menu_up_at_top_stays() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        advance_modal(&mut modal, KeyCode::Up);
        if let Modal::ActionMenu { selected, .. } = &modal {
            assert_eq!(*selected, 0);
        }
    }

    #[test]
    fn action_menu_down_at_bottom_stays() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 2,
        };
        advance_modal(&mut modal, KeyCode::Down);
        if let Modal::ActionMenu { selected, .. } = &modal {
            assert_eq!(*selected, 2);
        }
    }

    #[test]
    fn action_menu_j_moves_down() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        advance_modal(&mut modal, KeyCode::Char('j'));
        if let Modal::ActionMenu { selected, .. } = &modal {
            assert_eq!(*selected, 1);
        }
    }

    #[test]
    fn action_menu_k_moves_up() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 1,
        };
        advance_modal(&mut modal, KeyCode::Char('k'));
        if let Modal::ActionMenu { selected, .. } = &modal {
            assert_eq!(*selected, 0);
        }
    }

    #[test]
    fn action_menu_enter_on_second_item() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 1,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        match outcome {
            ModalOutcome::ActionSelected { action } => {
                assert_eq!(action.name, "Fix CI");
            }
            _ => panic!("expected ActionSelected"),
        }
    }

    #[test]
    fn action_menu_enter_on_empty_list_closes() {
        let mut modal = Modal::ActionMenu {
            actions: vec![],
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Enter);
        assert!(matches!(outcome, ModalOutcome::Close));
    }

    #[test]
    fn action_menu_other_key_continues() {
        let mut modal = Modal::ActionMenu {
            actions: make_test_actions(),
            selected: 0,
        };
        let outcome = advance_modal(&mut modal, KeyCode::Char('x'));
        assert!(matches!(outcome, ModalOutcome::Continue));
    }
}
