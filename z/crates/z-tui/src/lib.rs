/// z-tui: ratatui-based TUI frontend for z.
///
/// Three-panel layout:
///   - Left:   PROJECTS list (with ● active and 🌐 remote indicators)
///   - Right:  SESSIONS list for the selected project
///   - Bottom: STATUS bar with project info + keyboard hint strip
///
/// Navigation defaults to arrow keys; pass `Navigation::Vim` for hjkl.
use std::io;

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
        }
    }

    /// Returns (original_index, &entry) pairs filtered by the current search query.
    pub fn filtered_projects(&self) -> Vec<(usize, &ProjectEntry)> {
        let q = self.search_query.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| q.is_empty() || e.project.name.to_lowercase().contains(&q))
            .collect()
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
                let session_count = self
                    .selected_entry()
                    .map(|e| e.sessions.len())
                    .unwrap_or(0);
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
        terminal.draw(|f| render(f, state))?;

        if let Event::Key(key) = event::read()? {
            // ── Search mode ────────────────────────────────────────────────
            if state.search_mode {
                match key.code {
                    KeyCode::Esc => {
                        state.search_mode = false;
                        state.search_query.clear();
                        state.selected_project = 0;
                    }
                    KeyCode::Enter => {
                        state.search_mode = false;
                    }
                    KeyCode::Backspace => {
                        state.search_query.pop();
                        state.selected_project = 0;
                    }
                    KeyCode::Char(c) => {
                        state.search_query.push(c);
                        state.selected_project = 0;
                    }
                    _ => {}
                }
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
                        if let Some(entry) = state.selected_entry() {
                            let project = entry.project.name.clone();
                            let session =
                                if state.focused_panel == Panel::Sessions
                                    && !entry.sessions.is_empty()
                                {
                                    entry
                                        .sessions
                                        .get(state.selected_session)
                                        .map(|s| s.name.clone())
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
                        if let Some(entry) = state.selected_entry() {
                            if let Some(session) =
                                entry.sessions.get(state.selected_session)
                            {
                                return Ok(TuiAction::Delete {
                                    session: session.name.clone(),
                                });
                            }
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
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Top-level render: splits the terminal area into main content and status bar.
pub fn render(f: &mut Frame, state: &TuiState) {
    let area = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[0]);

    render_projects(f, main[0], state);
    render_sessions(f, main[1], state);
    render_status(f, outer[1], state);
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

    let sessions: Vec<&Session> = state
        .selected_entry()
        .map(|e| e.sessions.iter().collect())
        .unwrap_or_default();

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

    // ── Rendering snapshot tests ──────────────────────────────────────────

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

    // ── State / navigation unit tests ─────────────────────────────────────

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
}
