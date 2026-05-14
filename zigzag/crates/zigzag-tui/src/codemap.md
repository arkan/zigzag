# z/crates/z-tui/src/

## Responsibility

Provides the full-screen terminal UI frontend for the `z` session manager. Implements an interactive, ratatui-based panel layout for browsing projects and Zellij sessions, previewing git/forge state, dispatching actions, and managing project lifecycle — all within the terminal alternate screen. Also hosts standalone TUI pickers for session switching, log viewing, and action selection (used in Zellij floating panes).

## Design

### Architectural Pattern — Monolithic Event-Loop with Trait-Based Adapters

Three source modules, no sub-crate dependency within `z-tui`:

- **`lib.rs`** — The monolith. Contains entry points, all rendering, event dispatch, modal state machines, form logic, and the massive `TuiState` struct. Uses a single `event_loop` with a 100 ms poll cycle that multiplexes key events, async channel drains (4 channels), and terminal redraws.
- **`preview_state.rs`** — Pure functions for state transitions (`Loading` → `Ready`/`Error`, and merging extra forge/Zellij data into `Ready`). Separated to isolate the `PreviewData` state machine.
- **`refresh.rs`** — Background session refresh merge logic. Full cursor-preservation semantics via `merge_refresh` + staleness guard via `should_apply_refresh`. Separated to keep the merge algorithm independently testable.

### Key Abstractions

| Abstraction | Role |
|---|---|
| `TuiState` | Single source of truth; owns entries, cursor positions, panel focus, preview pipeline state (4 `mpsc::Receiver` fields), modal stack, notifications, theme, leader-key state, and refresh state revision counter. |
| `TuiCallbacks` | Closure-based dependency injection for all side-effecting operations (prune, add/edit/delete project, kill session, reload entries). Enables testing without real I/O. |
| `PreviewDataSource` (trait) | Adapter trait for loading git preview and extra forge/Zellij data. `NoopPreviewDataSource` is the default (returns errors). Real implementation injected via `run_tui`. |
| `SessionRefresher` (trait from `z_core`) | Adapter for polling session/notification/activity state in a background thread. |
| `Modal` (enum) | Sum type for all overlay dialogs — each variant carries its own local state. The `advance_modal` pure function implements per-modal key dispatch. No modal stack, only one at a time. |
| `PreviewData` (enum) | Three-state machine: `Loading` → `Ready(GitInfo)` | `Error(String)`. Transitioned by channel poll results via `preview_state` helpers. |
| `Panel` (enum) + `Navigation` (enum) | `Panel::Projects`/`Sessions` for focus routing; `Navigation::Arrows`/`Vim` for key scheme selection. |

### Async Pattern — Opportunistic Channel Polling

No async runtime. Background work is `std::thread::spawn` with `std::sync::mpsc` channels. The event loop polls 4 channels each cycle via `try_recv`:

1. **`preview_rx`** — Git info (fast, local filesystem)
2. **`forge_rx`** — PR/CI/Zellij data (slow, network)
3. **`gh_rx`** — GitHub issue/PR list JSON (network, only when GhPicker modal is open)
4. **`refresh_rx`** — Session/notification/activity refresh (filesystem + network, rate-limited to 5 s intervals)

Dropped senders (thread panic) are handled: preview → `Error`, forge → silently discard, gh → discard, refresh → discard receiver.

### Preview Pipeline (Two-Phase)

Phase 1 (git): spawned immediately on cursor change. Phase 2 (forge): spawned concurrently. If forge data arrives before Phase 1 completes (`PreviewData::Loading`), it is silently discarded (merged only into `Ready`). `preview_state::apply_extra_preview_result` returns `false` for non-Ready targets.

### Refresh Pipeline (State Revision Guard)

`TuiState.state_revision` is a monotonic counter incremented on every reload. Background refresh threads capture the revision at spawn time. `refresh::should_apply_refresh` rejects results if:
- `current_revision != refresh_revision` (stale — state was reloaded since spawn)
- A modal is open (avoids disrupting form input)

`refresh::merge_refresh` preserves cursor position by name (project name, session name), falls back to clamped index if the name vanished, and sorts sessions by most-recent-attach using activity timestamps.

### Standalone Pickers (No `TuiState`)

Three functions operate without the full `TuiState`:

- **`run_switch_picker`** — Renders a filtered session list with age/notification columns. Returns `Option<String>`. Standalone `SwitchPickerState` with its own `render_switch_picker` and `switch_picker_event_loop`; the dashboard also embeds `SwitchPickerState` as `Modal::SwitchPicker`.
- **`run_log_viewer`** — Full-screen scrolled log display. Reuses `Modal::LogViewer` + `advance_modal`.
- **`run_action_picker`** — Full-screen action menu. Reuses `Modal::ActionMenu` + `advance_modal`.

All three own their terminal lifecycle (raw mode, alternate screen enter/leave).

### Form Handling (ProjectForm)

Four-field form (Path, Name, Host, Transport) with:
- Path-aware Tab completion (`complete_path` returns directory entries; longest-common-prefix for multi-match)
- Auto-fill Name from Path basename (suppressed by `name_was_modified` flag)
- Inline validation warnings (path existence, git repo check, required-field markers, transport enum check)
- `expand_tilde_path` for `~` prefix
- Shared `tab_advance_with_completion` logic for both `AddProject` and `EditProject` variants

### Rendering Architecture — ratatui Widget Tree

`render` splits terminal into 3 vertical sections:
1. **Main panels** (horizontal 30/70 split): `render_projects` (List with active/remote/notification badges), `render_sessions` (List with notification badges)
2. **Preview pane** (8 lines fixed): `render_preview` — Paragraph with branch/tracking/dirty, PR/CI/review/Zellij lines, recent commits
3. **Status bar** (4 lines fixed): `render_status` — project info/locality/session count (or one-shot status message) + keyboard hint strip

Modals are rendered last (on top) via `render_modal` dispatch. The `Clear` widget is rendered before each modal to punch through the existing content.

Theme is applied uniformly through `rgb_to_color` and `theme_style_to_style` conversions from `z_core::theme::Theme`.

## Data & Control Flow

### Main Entry Point (`run_tui`)
```
entries, nav, notifications, callbacks, preview_source, refresher, theme, global_actions, review_tool
    ↓
enable_raw_mode + EnterAlternateScreen
    ↓
TuiState::with_preview_source(...)
    ↓
state.gh_tx = mpsc::Sender (for GhPicker background gh calls)
state.theme/global_actions/... = injected values
    ↓
state.trigger_preview_load()  // kick off initial git fetch
    ↓
event_loop(&mut terminal, &mut state, &callbacks)
    ↓
disable_raw_mode + LeaveAlternateScreen
    ↓
return TuiAction
```

### Event Loop Cycle (`event_loop`)
```
loop {
    state.poll_preview()      // drain preview_rx → PreviewData::Ready|Error
    state.poll_forge()        // drain forge_rx → merge into Ready
    state.poll_gh()           // drain gh_rx → update GhPicker.items
    state.poll_refresh()      // drain refresh_rx → merge sessions/notifications
    state.trigger_refresh()   // spawn if 5s elapsed + no in-flight
    terminal.draw(|f| render(f, state))
    if leader expired → clear leader_pending
    event::poll(100ms)
    if Event::Key::Press:
        dismiss status_message
        if leader_pending → dispatch Alt+z leader combo
        elif modal → advance_modal(...) → handle ModalOutcome
        elif search_mode → update search_query / navigate
        else → normal mode dispatch (nav, switcher, open, add, edit, delete, prune, search, help, reorder)
    state.trigger_preview_load()  // after any navigation event
}
```

### TuiAction Routing
- **`Quit`**, **`Open`**, **`SwitchToSession`**, **`New`**, **`NewFromIssue`**, **`NewFromPr`**, **`EditPerRepoConfig`**, **`RunAction`**, **`RunWorkflow`** — returned from `event_loop` to the caller, which leaves the TUI and executes the action.
- **Modal outcomes** (`Submit`, `SubmitEdit`, `DeleteConfirmed`, `SessionDeleteConfirmed`) — processed in-place via `TuiCallbacks` closures + `reload_fn`. The TUI never leaves the alternate screen.
- **`DeleteConfirmed`** → `apply_delete_project` → `delete_project_fn` + `reload_fn` → `apply_reloaded_entries` + clamp cursor.
- **`Submit`** → `apply_add_project` → `add_project_fn` + `reload_fn` → `apply_reloaded_entries` + move cursor to new entry.
- **`WorkflowSelected`** / **`ActionSelected`** / **`NewBranch`** → `return Ok(TuiAction::...)` (leaves TUI).
- **`GhIssueSelected`** / **`GhPrSelected`** → `return Ok(TuiAction::NewFromIssue|NewFromPr)` (leaves TUI with GitHub data).

### Background Thread — Session Refresh
```
trigger_refresh:
    if refresh_rx is Some OR last_refresh.elapsed() < 5s → return
    clone refresher, projects, state_revision
    (tx, rx) = mpsc::channel
    spawn:
        sessions = refresher.fetch_all_sessions(&projects)
        notifications = refresher.fetch_notifications()
        activity = refresher.fetch_activity()
        tx.send(RefreshMessage { state_revision, data })

poll_refresh:
    try_recv on refresh_rx
    if message:
        if should_apply_refresh(current_revision, message.revision, modal_open):
            merge_refresh(&mut entries, &mut notifications, data, selected_project, selected_session)
            update cursor from MergeResult
```

### Background Thread — GhPicker
```
OpenGhPicker outcome:
    spawn:
        if host is Some → ssh "cd <path> && gh <cmd>"
        else → gh <cmd> in <path>
        tx.send(stdout)
    state.modal = GhPicker { loading: true }

poll_gh:
    try_recv on gh_rx
    if json:
        parse via z_core::gh::parse_gh_issues|parse_gh_prs
        map to GhPickerItem vec
        update modal.items, loading = false
```

## Integration

### Crate Dependencies

| Dependency | Usage |
|---|---|
| `z_core` | Domain types (`Project`, `Session`, `PullRequest`, `CiStatus`, `ReviewStatus`, `Transport`), traits (`SessionRefresher`, `ForgeClient`), action system (`ActionDef`, `ActionType`, `ResolvedAction`, `ActionEnv`, `ActionPreview`, `builtin_actions`, `merge_actions`, `resolve_actions`), theme (`Theme`, `ThemeStyle`, `Rgb`), domain helpers (`sanitize_branch_name`, `slugify`), GitHub JSON parsing (`gh::parse_gh_issues`, `gh::parse_gh_prs`), activity sorting (`activity::sort_sessions_by_recent_attach`) |
| `ratatui` | Full widget tree: `Terminal`, `Frame`, `Layout`, `Block`, `List`, `ListItem`, `ListState`, `Paragraph`, `Clear`, `CrosstermBackend`, styling primitives |
| `crossterm` | Terminal lifecycle: `enable_raw_mode`, `EnterAlternateScreen`, `event::poll`, `event::read`, `Event`, `KeyCode`, `KeyModifiers` |
| `std::sync::mpsc` | All async communication (4 channels) |
| `std::thread` | Background workers for preview, forge, refresh, gh fetching |

### Exported Public API

```rust
// Main TUI
pub fn run_tui(...) -> io::Result<TuiAction>
pub fn render(f: &mut Frame, state: &TuiState)
pub fn fuzzy_match(query: &str, target: &str) -> bool

// Standalone pickers
pub fn run_switch_picker(...) -> io::Result<Option<String>>
pub fn run_log_viewer(lines: Vec<String>) -> io::Result<()>
pub fn run_action_picker(actions: Vec<ResolvedAction>) -> io::Result<Option<ResolvedAction>>

// Types (pub)
pub struct TuiState { ... }
pub struct ProjectEntry { ... }
pub struct WorkflowInfo { ... }
pub enum Navigation { Arrows, Vim }
pub enum Panel { Projects, Sessions }
pub enum TuiAction { Quit, Open, New, NewFromIssue, NewFromPr, EditPerRepoConfig, RunAction, RunWorkflow }
pub struct TuiCallbacks<'a> { ... }
pub enum PreviewData { Loading, Ready(GitInfo), Error(String) }
pub trait PreviewDataSource: Send + Sync { ... }
pub struct GitInfo { ... }
pub struct PreviewContext { ... }
pub struct PreviewExtraData { ... }
pub struct ZellijInfo { ... }
pub struct CommitInfo { ... }
pub enum Modal { ... }
pub struct ProjectForm { ... }
pub struct FormField { ... }
pub enum GhPickerKind { Issue, Pr }
pub struct GhPickerItem { ... }
pub struct SwitchPickerState { ... }
```

### External Side Effects (via `TuiCallbacks` closures)

All mutations of persistent state go through closures injected by the caller (`bin/z`), never through the TUI directly:

- **`prune_fn(bool) → io::Result<String>`** — Remove orphan git worktrees
- **`log_fn(usize) → io::Result<Vec<String>>`** — Fetch recent log lines
- **`swap_fn(usize, usize) → io::Result<()>`** — Reorder projects in config
- **`kill_session_fn(&str) → io::Result<()>`** — Kill a Zellij session
- **`add_project_fn(...) → io::Result<()>`** — Append to `projects.kdl`
- **`edit_project_fn(...) → io::Result<()>`** — Update entry in `projects.kdl`
- **`delete_project_fn(&str) → io::Result<()>`** — Remove from `projects.kdl`
- **`reload_fn() → io::Result<(Vec<ProjectEntry>, HashSet<String>)>`** — Re-read all project/session state after mutation

### Background Side Effects

- **`PreviewDataSource`** — Reads git data (branch, status, log, commits) from filesystem; optional SSH for remote hosts. Network fetch for PR/CI/Zellij data.
- **`SessionRefresher`** — Polls Zellij daemon for session list and notification state.
- **GhPicker background thread** — Runs `gh issue list` or `gh pr list` either locally or via SSH.
