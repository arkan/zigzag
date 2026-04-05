/// z-tui: ratatui-based TUI frontend for z.
///
/// Layout (four sections):
///   - Top-left:    PROJECTS list (with ● active and 🌐 remote indicators)
///   - Top-right:   SESSIONS list for the selected project
///   - Middle:      PREVIEW pane — git branch / status / commits (async)
///   - Bottom:      STATUS bar with project info + keyboard hint strip
///
/// Navigation defaults to arrow keys; pass `Navigation::Vim` for hjkl.
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
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use z_core::domain::{Project, Session};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A project with its active Zellij sessions pre-loaded.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub project: Project,
    pub sessions: Vec<Session>,
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

/// Git information fetched asynchronously for the selected project/session.
#[derive(Debug, Clone, PartialEq)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub is_dirty: bool,
    pub commits: Vec<CommitInfo>,
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
    pub fn trigger_preview_load(&mut self) {
        let Some(key) = self.current_preview_key() else {
            return;
        };
        if key == self.preview_key {
            return; // already loading or loaded for this key
        }

        let path: PathBuf = self
            .selected_entry()
            .map(|e| e.project.path.clone())
            .unwrap_or_default();

        self.preview_key = key;
        self.preview_data = PreviewData::Loading;

        let (tx, rx) = mpsc::channel();
        self.preview_rx = Some(rx);

        std::thread::spawn(move || {
            let result = fetch_git_info(&path.to_string_lossy());
            let _ = tx.send(result);
        });
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
// Entry point
// ---------------------------------------------------------------------------

/// Launch the full-screen ratatui TUI.
///
/// Sets up the terminal, runs the event loop, restores the terminal on exit,
/// and returns the action the user chose.
pub fn run_tui(entries: Vec<ProjectEntry>, navigation: Navigation) -> io::Result<TuiAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new(entries, navigation);
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
        // Check if async preview data has arrived.
        state.poll_preview();

        terminal.draw(|f| render(f, state))?;

        // Poll with a short timeout so we can refresh the preview pane
        // without waiting for a keypress.
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
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
            Constraint::Length(3),  // status bar
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
        .map(|s| ListItem::new(s.name.as_str()))
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

    let hints = " [o]pen  [n]ew  [d]elete  [p]rune  [a]utopilot  [/]search  [q]uit";
    let content = format!("{}\n{}", project_info, hints);

    let paragraph = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" STATUS "));
    f.render_widget(paragraph, area);
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
            },
            ProjectEntry {
                project: make_project("hermes", false),
                sessions: vec![],
            },
            ProjectEntry {
                project: make_project("prod-api", true),
                sessions: vec![],
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
}
