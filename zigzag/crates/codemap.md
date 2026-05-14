# zigzag/crates/

<!--
  Codemap for the entire crate collection.
  Each sub-crate has its own codemap; this file documents the collection as a whole.
-->

## Responsibility

The `crates/` directory contains the entire `zigzag` project as a **multi-crate workspace**.
Each crate is a focused, independently-versioned library (or binary) that composes
to form the `zigzag` CLI — a Zellij-based project manager for developers.

**Collection-level job**: Organise the system into clean architectural layers so that
I/O-agnostic business logic, UI rendering, workflow automation, and future frontends
(zigzag-web, zigzag-plugin) are fully decoupled.

### Crate Dependency Graph

```
zigzag-core ─────────────────────────────────────────┐
  │                                              │
  ├── zigzag-tui  (depends on zigzag-core + ratatui) ──────┤
  │                                              │
  ├── zigzag-autopilot  (depends on zigzag-core) ──────────┤
  │                                              │
  ├── zigzag-cli  (depends on zigzag-core, zigzag-tui,          │
  │           zigzag-autopilot) ◄── produces `zigzag` bin  │
  │                                              │
  ├── zigzag-web  (stub, depends on zigzag-core) ──────────┤
  │                                              │
  └── zigzag-plugin  (stub, depends on zigzag-core) ───────┘

Key: no crate depends on zigzag-cli, zigzag-web, or zigzag-plugin.
     zigzag-core is the sole foundation — zero reverse dependencies.
```

### Crate Roles

| Crate | Role | Dependencies | Phase |
|-------|------|-------------------|-------|
| `zigzag-core` | I/O-agnostic business logic, domain types, traits | none | stable |
| `zigzag-tui` | ratatui-based TUI frontend | zigzag-core | stable |
| `zigzag-autopilot` | State-machine workflow engine | zigzag-core | phase 6 |
| `zigzag-cli` | Binary entry point — CLI commands + adapter wiring | zigzag-core, zigzag-tui, zigzag-autopilot | stable |
| `zigzag-web` | Future web server with axum | zigzag-core | phase 5 (stub) |
| `zigzag-plugin` | Future WASM Zellij plugin | zigzag-core | phase 4 (stub) |

---

## Design Patterns

### 1. Trait-Based I/O Abstraction

All side-effecting operations are behind traits defined in `zigzag-core::traits`:

- `ProjectStore` / `ProjectStoreWriter` — CRUD for projects
- `SessionManager` — Zellij session lifecycle
- `WorktreeManager` — git worktree operations (delegated to `wt` CLI)
- `ForgeClient` — PR/CI/review queries (delegated to `gh` CLI)
- `Notifier` — out-of-band notifications
- `SessionRefresher` — async session poll for TUI background refresh
- `ActivityStore` — session attach-timestamp persistence
- `WorktreeMetadataStore` — worktree-first metadata, pending notifications, and agent status

**Adapters** live in `zigzag-cli` (concrete implementations that shell out to
`zellij`, `wt`, `gh`, the filesystem, etc.). This means `zigzag-core` has
zero I/O — it is purely data structures, parsing, and logic.
`zigzag-core` is fully testable without any external process.

### 2. Three-Tier Config Merging

Configuration cascades: **hardcoded default < global config < per-repo config**.

- **Hardcoded defaults**: `zigzag-core::layout::default_layout()` — two-tab layout
  (claude + shell)
- **Global config**: `~/.config/zigzag/config.kdl` — KDL file parsed by
  `zigzag-core::config::parse_global_config_kdl()`
- **Per-repo config**: `<project-path>/.config/zigzag.kdl` — KDL file parsed by
  `zigzag-core::config::parse_per_repo_config_kdl()`

The lowest tier wins **entirely** — no partial merge (e.g. per-repo layout
completely replaces global layout).

Same pattern for prompt templates (`issue-prompt-template`,
`pr-prompt-template`), actions, and autopilot config.

### 3. Callback-Driven TUI Mutation

The TUI (`zigzag-tui`) never directly performs I/O. It receives a `TuiCallbacks`
struct of function pointers — closures provided by `zigzag-cli` — that it invokes
for side effects (add/edit/delete project, kill session, prune, reload).
Operations that require leaving the alternate screen
(e.g. `zellij attach-session`) return a `TuiAction` variant instead.

This keeps `zigzag-tui` pure UI logic — no knowledge of KDL, Zellij, or the
filesystem.

### 4. Two-Phase Async Preview Loading

When a project/session is selected in the TUI, preview data loads in two
async phases:

1. **Fast (git info)**: branch, ahead/behind, dirty flag, recent commits —
   spawned first, updates the preview pane as soon as it arrives
2. **Slow (forge data)**: PR, CI status, Zellij session info, review
   status — spawned second, merged into existing `GitInfo` when complete

Both use `std::sync::mpsc` channels back to the TUI event loop. The TUI
polls both channels every frame via `poll_preview()` and `poll_forge()`.

### 5. Session Refresh Background Loop

The TUI spawns a background thread every 5 seconds that re-queries
`zellij list-sessions` and notification files across all projects.
Results are merged via `refresh::merge_refresh()` only when no modal is
open, preventing disruption of in-progress forms. A `state_revision`
counter prevents stale refreshes from overwriting newer reloads.

### 6. Session Name Convention

Zellij sessions follow the naming convention `{project}:{branch}` where
`/` in branch names is replaced with `-` (via `sanitize_branch_name()`).
This allows unambiguous parsing back to `(project, branch)` pairs.

### 7. Action System (Configurable + Conditional)

Actions are KDL-defined commands that appear in the TUI action menu.
Features:
- **Three layers**: builtin < global config < per-repo config (override by name)
- **Conditions**: `always`, `has_pr`, `has_ci_failure`, `has_new_comments`
- **Contexts**: `project` (no session needed) or `session` (requires active branch)
- **Pane types**: `float`, `float-fullscreen`, `split`, `tab`
- **Variable interpolation**: `${project}`, `${branch}`, `${pr_number}`, `${review_tool}`, etc.
- **Disable**: actions can be marked `disabled: true` to remove them from the menu

### 8. Activity-Based MRU Sorting

Session attach timestamps are persisted to a file via `ActivityStore`.
The TUI and `zigzag switch` picker use these timestamps to sort sessions with
the most recently attached session first. Sessions with no recorded
activity sort to the end (stable relative order).

---

## Data & Control Flow

### Top-Level CLI Dispatch

```
main()
  ├── depcheck (ProcessDepChecker) — verify zellij, wt, gh are installed
  └── run()
       ├── no args           → cmd_tui() → TUI loop
       ├── "list"            → cmd_list() → print project/session summary
       ├── "open"            → cmd_open() → session create/attach
       ├── "close"           → cmd_close() → zellij detach
       ├── "delete"          → cmd_delete() → kill session + prompt worktree removal
       ├── "prune"           → cmd_prune() → find + remove orphaned sessions/worktrees
       ├── "notify"          → cmd_notify() → write notification + dispatch
       ├── "autopilot"       → cmd_autopilot_dispatch() → workflow execution
       ├── "logs"            → cmd_logs() → print recent log entries
       ├── "switch"          → cmd_switch() → TUI session switcher
       ├── "logs-viewer"     → cmd_logs_viewer() → TUI log viewer
       └── "actions"         → cmd_actions() → action picker in floating pane
```

### TUI Event Loop (cmd_tui)

```
build_entries()
  ├── KdlProjectStore.list_projects()
  ├── zellij list-sessions (one subprocess for all)
  ├── For each project: filter sessions, count worktrees, load repo config
  └── build ProjectEntry with sessions + workflows + actions

loop {
  render(entries, notifications, preview, modals)
  handle_input():
    ├── navigation (up/down/tab) → move between projects/sessions
    ├── search (/) → fuzzy filter projects/sessions
    ├── action keys:
    │   ├── o/Enter → Open (stays in TUI for session, leaves for new project)
    │   ├── n → New session menu (blank / from issue / from PR)
    │   ├── d → Delete session
    │   ├── a → Workflow selector
    │   ├── e → Edit per-repo config
    │   ├── p → Prune
    │   ├── l → Log viewer
    │   ├── r → Actions menu
    │   ├── ? → Help
    │   └── q → Quit
    ├── modals → advance_modal()
    └── trigger_preview_load() + poll_preview() + poll_forge() + trigger_refresh()
}
```

### Session Open Flow

```
cmd_open(project, branch, prompt)
  ├── KdlProjectStore.get_project(project)
  ├── ZellijSessionManager.list_sessions(project)
  ├── session_open::plan_open_session() → existing or new session
  ├── If existing → attach_session()
  ├── If new:
  │   ├── WtWorktreeManager: find or create worktree for branch
  │   ├── Merge layout: hardcoded < global < per-repo
  │   ├── Inject ZIGZAG_SESSION_NAME env + optional prompt
  │   ├── Inject Claude stop hook (settings.json)
  │   ├── Apply theme
  │   └── ZellijSessionManager.create_session() with KDL layout
  └── Record activity + clear notifications
```

**Remote variant** (project has `host`):
- SSH into host, run `zigzag open <project> <branch>` remotely via
  `ssh -t` or `mosh`

### Prune Flow

```
cmd_prune()
  For each project:
    ├── ZellijSessionManager.list_sessions(project)
    ├── WtWorktreeManager.list_worktrees(project)
    ├── prune::find_orphaned_sessions(sessions, worktrees)
    │     → sessions whose branch has no matching worktree
    └── prune::find_orphaned_worktrees(worktrees, sessions)
          → worktrees whose branch has no active session
                         (excluding main/master)
  Preview → confirm → kill sessions + remove worktrees
```

### Autopilot Workflow Execution

```
cmd_autopilot_run(project, workflow_name)
  ├── Resolve workflow definition (builtin + per-repo custom)
  ├── load_or_start_run() — resume in-progress or start fresh
  └── execute_workflow_run()
        loop:
          ├── Execute current step (Run/Notify/Confirm)
          ├── Retry on failure (up to max_retries)
          ├── Advance to next step on success
          ├── Persist WorkflowRun state
          └── Continue until terminal step or step limit
```

---

## Integration Points

### External CLI Dependencies (shelled out by zigzag-cli adapters)

| Tool | Used By | Purpose |
|------|---------|---------|
| `zellij` | ZellijSessionManager | Session CRUD, layout, actions |
| `wt` (worktrunk) | WtWorktreeManager | Git worktree management |
| `gh` | GhForgeClient, preview | PR/CI/review queries, issue/PR list |
| `ssh` / `mosh` | remote module | Remote session proxying |
| `git` | CliPreviewDataSource (via CLI) | Branch, status, commit log |

### Storage (Filesystem)

| Path | Format | Content | Adapter |
|------|--------|---------|---------|
| `~/.config/zigzag/config.kdl` | KDL | Global config (layout, deps, notifications, actions) | `zigzag-core::config` |
| `~/.config/zigzag/projects.kdl` | KDL | Project registry (name, path, host, transport) | `KdlProjectStore` |
| `~/.config/zigzag/worktree-metadata.json` | JSON | Worktree metadata, pending notifications, LLM status | `LocalWorktreeMetadataStore` / `RemoteWorktreeMetadataStore` |
| `<project>/.config/zigzag.kdl` | KDL | Per-repo config (layout, deploy, autopilot, actions) | `zigzag-core::config` |
| `~/.config/zigzag/session-activity.json` | JSON | Session attach timestamps | `FileActivityStore` |
| `~/.local/state/zigzag/zigzag.log` | TSV | Structured event log | `FileLogger` |
| `~/.local/share/zigzag/autopilot/*.json` | JSON | In-progress workflow run state | `RunStore` (in zigzag-autopilot) |

### Trait Contracts Between Crates

```
zigzag-core::traits::ProjectStore ────────────── KdlProjectStore (zigzag-cli)
zigzag-core::traits::SessionManager ──────────── ZellijSessionManager (zigzag-cli)
zigzag-core::traits::WorktreeManager ─────────── WtWorktreeManager (zigzag-cli)
zigzag-core::traits::ForgeClient ─────────────── GhForgeClient (zigzag-cli)
zigzag-core::traits::Notifier ────────────────── DispatchNotifier (zigzag-cli)
zigzag-core::traits::SessionRefresher ────────── ZellijSessionRefresher (zigzag-cli)
zigzag-core::activity::ActivityStore ─────────── FileActivityStore (zigzag-cli)
zigzag-core::traits::WorktreeMetadataStore ───── LocalWorktreeMetadataStore / RemoteWorktreeMetadataStore (zigzag-cli)

zigzag-tui::PreviewDataSource ────────────────── CliPreviewDataSource (zigzag-cli)
zigzag-tui::TuiCallbacks ─────────────────────── closures (zigzag-cli main.rs)

zigzag-autopilot::run_loop::RunStore ─────────── (zigzag-cli, in autopilot_runner.rs)
```

### Stub Crates (Future)

- **`zigzag-plugin`** (phase 4): WASM Zellij plugin — will embed zigzag-core logic
  into a Zellij plugin running inside the Zellij WASM runtime
- **`zigzag-web`** (phase 5): axum-based web server — will serve the same
  project management capabilities over HTTP, potentially with a WASM-compiled
  ratatui frontend

Both stubs depend on `zigzag-core` and will reuse its domain types and traits,
with new adapters for their respective environments.
