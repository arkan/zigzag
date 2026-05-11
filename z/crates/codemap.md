# z/crates/

<!--
  Codemap for the entire crate collection.
  Each sub-crate has its own codemap; this file documents the collection as a whole.
-->

## Responsibility

The `crates/` directory contains the entire `z` project as a **multi-crate workspace**.
Each crate is a focused, independently-versioned library (or binary) that composes
to form the `z` CLI — a Zellij-based project manager for developers.

**Collection-level job**: Organise the system into clean architectural layers so that
I/O-agnostic business logic, UI rendering, workflow automation, and future frontends
(z-web, z-plugin) are fully decoupled.

### Crate Dependency Graph

```
z-core ─────────────────────────────────────────┐
  │                                              │
  ├── z-tui  (depends on z-core + ratatui) ──────┤
  │                                              │
  ├── z-autopilot  (depends on z-core) ──────────┤
  │                                              │
  ├── z-cli  (depends on z-core, z-tui,          │
  │           z-autopilot) ◄── produces `z` bin  │
  │                                              │
  ├── z-web  (stub, depends on z-core) ──────────┤
  │                                              │
  └── z-plugin  (stub, depends on z-core) ───────┘

Key: no crate depends on z-cli, z-web, or z-plugin.
     z-core is the sole foundation — zero z-dependencies.
```

### Crate Roles

| Crate | Role | Dependencies (z) | Phase |
|-------|------|-------------------|-------|
| `z-core` | I/O-agnostic business logic, domain types, traits | none | stable |
| `z-tui` | ratatui-based TUI frontend | z-core | stable |
| `z-autopilot` | State-machine workflow engine | z-core | phase 6 |
| `z-cli` | Binary entry point — CLI commands + adapter wiring | z-core, z-tui, z-autopilot | stable |
| `z-web` | Future web server with axum | z-core | phase 5 (stub) |
| `z-plugin` | Future WASM Zellij plugin | z-core | phase 4 (stub) |

---

## Design Patterns

### 1. Trait-Based I/O Abstraction

All side-effecting operations are behind traits defined in `z-core::traits`:

- `ProjectStore` / `ProjectStoreWriter` — CRUD for projects
- `SessionManager` — Zellij session lifecycle
- `WorktreeManager` — git worktree operations (delegated to `wt` CLI)
- `ForgeClient` — PR/CI/review queries (delegated to `gh` CLI)
- `Notifier` — out-of-band notifications
- `SessionRefresher` — async session poll for TUI background refresh
- `ActivityStore` — session attach-timestamp persistence
- `WorktreeMetadataStore` — worktree-first metadata, pending notifications, and agent status

**Adapters** live in `z-cli` (concrete implementations that shell out to
`zellij`, `wt`, `gh`, the filesystem, etc.). This means `z-core` has
zero I/O — it is purely data structures, parsing, and logic.
`z-core` is fully testable without any external process.

### 2. Three-Tier Config Merging

Configuration cascades: **hardcoded default < global config < per-repo config**.

- **Hardcoded defaults**: `z-core::layout::default_layout()` — two-tab layout
  (claude + shell)
- **Global config**: `~/.config/z/config.kdl` — KDL file parsed by
  `z-core::config::parse_global_config_kdl()`
- **Per-repo config**: `<project-path>/.config/z.kdl` — KDL file parsed by
  `z-core::config::parse_per_repo_config_kdl()`

The lowest tier wins **entirely** — no partial merge (e.g. per-repo layout
completely replaces global layout).

Same pattern for prompt templates (`issue-prompt-template`,
`pr-prompt-template`), actions, and autopilot config.

### 3. Callback-Driven TUI Mutation

The TUI (`z-tui`) never directly performs I/O. It receives a `TuiCallbacks`
struct of function pointers — closures provided by `z-cli` — that it invokes
for side effects (add/edit/delete project, kill session, prune, reload).
Operations that require leaving the alternate screen
(e.g. `zellij attach-session`) return a `TuiAction` variant instead.

This keeps `z-tui` pure UI logic — no knowledge of KDL, Zellij, or the
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
The TUI and `z switch` picker use these timestamps to sort sessions with
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
  │   ├── Inject Z_SESSION_NAME env + optional prompt
  │   ├── Inject Claude stop hook (settings.json)
  │   ├── Apply theme
  │   └── ZellijSessionManager.create_session() with KDL layout
  └── Record activity + clear notifications
```

**Remote variant** (project has `host`):
- SSH into host, run `z open <project> <branch>` remotely via
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

### External CLI Dependencies (shelled out by z-cli adapters)

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
| `~/.config/z/config.kdl` | KDL | Global config (layout, deps, notifications, actions) | `z-core::config` |
| `~/.config/z/projects.kdl` | KDL | Project registry (name, path, host, transport) | `KdlProjectStore` |
| `~/.config/z/worktree-metadata.json` | JSON | Worktree metadata, pending notifications, LLM status | `LocalWorktreeMetadataStore` / `RemoteWorktreeMetadataStore` |
| `<project>/.config/z.kdl` | KDL | Per-repo config (layout, deploy, autopilot, actions) | `z-core::config` |
| `~/.local/share/z/activity.json` | JSON | Session attach timestamps | `FileActivityStore` |
| `~/.local/share/z/logs/*.log` | TSV | Structured event log | `FileLogger` |
| `~/.local/share/z/autopilot/*.json` | JSON | In-progress workflow run state | `RunStore` (in z-autopilot) |

### Trait Contracts Between Crates

```
z-core::traits::ProjectStore ────────────── KdlProjectStore (z-cli)
z-core::traits::SessionManager ──────────── ZellijSessionManager (z-cli)
z-core::traits::WorktreeManager ─────────── WtWorktreeManager (z-cli)
z-core::traits::ForgeClient ─────────────── GhForgeClient (z-cli)
z-core::traits::Notifier ────────────────── DispatchNotifier (z-cli)
z-core::traits::SessionRefresher ────────── ZellijSessionRefresher (z-cli)
z-core::activity::ActivityStore ─────────── FileActivityStore (z-cli)
z-core::traits::WorktreeMetadataStore ───── LocalWorktreeMetadataStore / RemoteWorktreeMetadataStore (z-cli)

z-tui::PreviewDataSource ────────────────── CliPreviewDataSource (z-cli)
z-tui::TuiCallbacks ─────────────────────── closures (z-cli main.rs)

z-autopilot::run_loop::RunStore ─────────── (z-cli, in autopilot_runner.rs)
```

### Stub Crates (Future)

- **`z-plugin`** (phase 4): WASM Zellij plugin — will embed z-core logic
  into a Zellij plugin running inside the Zellij WASM runtime
- **`z-web`** (phase 5): axum-based web server — will serve the same
  project management capabilities over HTTP, potentially with a WASM-compiled
  ratatui frontend

Both stubs depend on `z-core` and will reuse its domain types and traits,
with new adapters for their respective environments.
