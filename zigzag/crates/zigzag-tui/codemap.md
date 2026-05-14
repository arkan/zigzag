# zigzag/crates/zigzag-tui/

## Responsibility

Provides the full-screen terminal UI frontend for the `zigzag` session manager. This crate implements an interactive, ratatui-based panel layout for browsing projects and Zellij sessions, previewing git/forge state, dispatching actions, and managing project lifecycle — all within the terminal alternate screen. It also hosts standalone TUI pickers (session switch, log viewer, action menu) used in Zellij floating panes.

Boundary: consumes domain types and traits from `zigzag-core`, owns all TUI rendering and user interaction logic, and delegates persistent state mutations to callbacks injected by the binary (`bin/zigzag`). Never writes to disk or calls Zellij directly.

## Design

### Module Layout

| Module | Contents |
|---|---|
| `lib.rs` (monolithic) | Entry points (`run_tui`, `run_switch_picker`, `run_log_viewer`, `run_action_picker`), all rendering functions, event loop, `TuiState` struct, modal state machines, form logic, `TuiCallbacks` closure type, `TuiAction` enum, `Modal` enum, `ProjectForm`, `GhPickerItem`, fuzzy matching |
| `preview_state.rs` (pub in crate) | Pure state transitions for the `PreviewData` enum (`Loading` → `Ready`/`Error`, merging extra data into `Ready`); separated to isolate the preview pipeline state machine |
| `refresh.rs` (pub in crate) | Background session refresh merge logic — `merge_refresh` for cursor-preserving state merging, `should_apply_refresh` for staleness guard; separated for independent testability |

### Key Patterns

- **Synchronous event loop, async via threads**: No async runtime. Background work spawns `std::thread` with `std::sync::mpsc` channels. The event loop polls 4 channels each cycle via `try_recv` (preview, forge, gh, refresh).
- **Closure-based dependency injection**: `TuiCallbacks` is a struct of closure references for all side-effecting operations (add/edit/delete project, kill session, prune, reload). Enables testing without real I/O and keeps the TUI pure.
- **Trait-based adapters at crate boundary**: `PreviewDataSource` trait for git/forge/Zellij data loading (default: `NoopPreviewDataSource` returns errors); `SessionRefresher` trait (from `zigzag-core`) for polling daemon state. Injected at `run_tui` call site.
- **Three-source code split**: `lib.rs` (monolith), `preview_state.rs` (preview state machine), `refresh.rs` (refresh merge). No sub-crate dependencies within `zigzag-tui`.
- **State revision guard**: `TuiState.state_revision` is a monotonic counter incremented on every reload. Background refresh threads capture the revision at spawn; `should_apply_refresh` rejects stale results.
- **Modal enum with local state**: Each `Modal` variant carries its own state. `advance_modal` is a pure function mapping (modal, key) → outcome. Only one modal at a time (no modal stack).

## Flow

### Main entry — `run_tui`
```
caller injects: entries, nav, notifications, TuiCallbacks, PreviewDataSource, SessionRefresher, theme, global_actions, review_tool
    ↓
enable_raw_mode + EnterAlternateScreen
    ↓
TuiState::with_preview_source(...) — owns all state
    ↓
event_loop(&mut terminal, &mut state, &callbacks) — 100ms poll cycle
    ↓
disable_raw_mode + LeaveAlternateScreen
    ↓
return TuiAction (Quit | Open { project, session } | SwitchToSession { session } | New { project, branch } | NewFromIssue/Pr | EditPerRepoConfig | RunAction | RunWorkflow)
```

### Event loop cycle
```
each iteration:
  1. drain 4 mpsc channels (preview, forge, gh, refresh)
  2. trigger refresh if 5s elapsed + no in-flight
  3. terminal.draw(render) — 3-section layout (panels, preview, status) + optional modal overlay
  4. event::poll(100ms) → dispatch: leader key, modal, search, or normal-mode navigation
  5. trigger preview load after cursor movement
```

### Standalone pickers
Three functions (`run_switch_picker`, `run_log_viewer`, `run_action_picker`) manage their own terminal lifecycle and render without `TuiState`. They live in `lib.rs`; the dashboard also reuses the switch picker renderer as `Modal::SwitchPicker`.

### Preview pipeline (two-phase)
1. Phase 1 (git info) — spawned on cursor change
2. Phase 2 (forge/Zellij) — spawned concurrently; silently discarded if Phase 1 is still `Loading`
Merge only applies to `PreviewData::Ready` target; `preview_state::apply_extra_preview_result` returns `false` otherwise.

## Integration

### Crate graph
```
zigzag-tui
  ├── zigzag-core (domain types, traits, actions, theme, gh parsing, activity sorting)
  ├── ratatui (Terminal, Frame, Layout, List, Paragraph, Block, Clear, styling)
  └── crossterm (raw mode, alternate screen, event poll/read, KeyCode, KeyModifiers)
```

No reverse dependencies — `zigzag-tui` is a leaf crate consumed only by the `bin/zigzag` binary.

### Public API (crate boundary)

All public items exported from `lib.rs`:

**Entry points:**
- `pub fn run_tui(...) -> io::Result<TuiAction>` — main TUI event loop
- `pub fn render(f: &mut Frame, state: &TuiState)` — draw current state (used by tests)
- `pub fn fuzzy_match(query: &str, target: &str) -> bool` — fuzzy filter helper
- `pub fn run_switch_picker(...) -> io::Result<Option<String>>` — standalone session picker; dashboard `s` / `Alt+k` uses the same picker state as a modal overlay
- `pub fn run_log_viewer(lines: Vec<String>) -> io::Result<()>` — standalone log viewer
- `pub fn run_action_picker(actions: Vec<ResolvedAction>) -> io::Result<Option<ResolvedAction>>` — standalone action menu

**Key types:**
- `TuiState`, `ProjectEntry`, `WorkflowInfo` — state containers
- `Navigation` (Arrows/Vim), `Panel` (Projects/Sessions) — configuration enums
- `TuiAction` — action returned to caller after TUI exits
- `TuiCallbacks` — closure DI struct for all side effects
- `PreviewData` (Loading/Ready/Error), `GitInfo`, `PreviewContext`, `PreviewExtraData`, `ZellijInfo`, `CommitInfo` — preview pipeline types
- `PreviewDataSource` trait — adapter for loading preview data
- `Modal` enum — all modal overlay variants with local state
- `ProjectForm`, `FormField` — form state
- `GhPickerKind`, `GhPickerItem` — GitHub picker types
- `SwitchPickerState` — standalone switch picker state

### External side effects (injected closures)

All mutations of persistent state go through `TuiCallbacks` closures, never called directly by the TUI:
- `prune_fn(bool) -> io::Result<String>` — remove orphan git worktrees
- `log_fn(usize) -> io::Result<Vec<String>>` — fetch recent log lines
- `swap_fn(usize, usize) -> io::Result<()>` — reorder projects in config
- `kill_session_fn(&str) -> io::Result<()>` — kill a Zellij session
- `add_project_fn(...) -> io::Result<()>` — append to projects.kdl
- `edit_project_fn(...) -> io::Result<()>` — update entry in projects.kdl
- `delete_project_fn(&str) -> io::Result<()>` — remove from projects.kdl
- `reload_fn() -> io::Result<(Vec<ProjectEntry>, HashSet<String>)>` — re-read all state after mutation

### Background side effects (via traits)
- `PreviewDataSource` — reads git data from filesystem; SSH for remote hosts; network fetch for PR/CI/Zellij
- `SessionRefresher` (from zigzag-core) — polls Zellij daemon for sessions/notifications/activity
- GhPicker background thread — runs `gh issue list` / `gh pr list` locally or via SSH

### See also
- `src/codemap.md` — detailed internal architecture (state machine details, rendering widget tree, modal dispatch, form validation, standalone picker internals)
