# Zigzag — Workspace Codemap

**Version:** 0.6.0
**Edition:** 2021  
**Resolvers:** `resolver = "2"`

---

## Responsibility

`zigzag` is a developer productivity tool that bridges **Zellij terminal multiplexer sessions** with **git worktrees** and **GitHub workflow automation**. It manages the lifecycle of per-branch Zellij sessions backed by git worktrees, provides a TUI for browsing projects/sessions with async PR/CI preview, and runs automated CI-fix/review/merge workflows via a built-in state machine.

The workspace is a **Rust monorepo** with 6 crates organized in a layered dependency graph:

```
                ┌──────────┐
                │  zigzag-cli   │  Binary entry point
                ├──────────┤
                │ zigzag-tui    │  Ratatui-based terminal UI
                ├──────────┤
                │zigzag-autopilot│ Workflow automation engine
                ├──────────┤
     ┌──────────┤  zigzag-core  ├──────────┐
     │          └──────────┘          │
     ▼                                ▼
  zigzag-plugin (stub)                zigzag-web (stub)
  WASM Zellij plugin             Axum web server
```

- **zigzag-core** — Foundational types, trait interfaces, config parsing, domain logic. No internal crate deps.
- **zigzag-tui** — Full-screen terminal UI using `ratatui` + `crossterm`. Depends on zigzag-core for types/domains.
- **zigzag-cli** — Binary crate. Depends on zigzag-core, zigzag-tui, zigzag-autopilot. Provides concrete implementations of all zigzag-core traits.
- **zigzag-autopilot** — Workflow DSL parsing, state machine, run loop, persistence. Depends on zigzag-core.
- **zigzag-plugin** — WASM plugin for Zellij (stub, future phase).
- **zigzag-web** — Web server with axum + ratatui WASM (stub, future phase).

---

## Design Patterns

### 1. Trait-Based Plugin Architecture

zigzag-core defines **abstraction traits**; zigzag-cli provides **concrete adapters**. This keeps core logic testable and crate boundaries clean.

| Trait | Purpose | CLI Adapter |
|-------|---------|-------------|
| `ProjectStore` / `ProjectStoreWriter` | CRUD for configured projects | `KdlProjectStore` (file-backed KDL) |
| `SessionManager` | Zellij session lifecycle | `ZellijSessionManager` (subprocess) |
| `WorktreeManager` | Git worktree lifecycle | `WtWorktreeManager` (`wt` CLI) |
| `ForgeClient` | GitHub PR/CI/review data | `GhForgeClient` (`gh` CLI) |
| `Notifier` | External notification dispatch | `DispatchNotifier` (macos + telegram) |
| `SessionRefresher` | Async TUI state refresh | `ZellijSessionRefresher` |
| `DepChecker` | External tool version probing | `ProcessDepChecker` |
| `ActivityStore` | Persisted attach timestamps | `FileActivityStore` |
| `WorktreeMetadataStore` | Worktree-first metadata, notifications, and agent status | `LocalWorktreeMetadataStore` / `RemoteWorktreeMetadataStore` |

### 2. Callback-Based TUI

The `zigzag-tui` crate is **pure UI** with no direct side effects. It accepts a `TuiCallbacks` struct of closures for all mutating operations (prune, kill session, add/edit/delete project, reload). The event loop returns a `TuiAction` enum for operations that require leaving the alternate screen (open session, run workflow, edit config).

### 3. Background Async via mpsc Channels

The TUI spawns OS threads for three concurrent data pipelines:

| Pipeline | Trigger | Receiver | Data |
|----------|---------|----------|------|
| Git preview | Selection change | `preview_rx` → `poll_preview()` | branch, status, commits |
| Forge/Zellij | After git preview | `forge_rx` → `poll_forge()` | PR, CI, review, session uptime |
| Session refresh | Every 5 seconds | `refresh_rx` → `poll_refresh()` | session list, notifications, activity |

All polling happens in the main event loop tick before `terminal.draw()`.

### 4. KDL Configuration

All configuration uses the KDL document format:
- **Global:** `~/.config/zigzag/config.kdl` — theme, navigation, notifications, dependencies, actions, default layout
- **Per-repo:** `<project>/.config/zigzag.kdl` — layout override, deploy command, autopilot settings, repo actions
- **Projects list:** `~/.config/zigzag/projects.kdl` — project entries

### 5. State Machine (zigzag-autopilot)

Autopilot workflows are defined as KDL documents with named steps, each with:
- An action (`run`, `notify`, `confirm`)
- Transition rules (`on-success`, `on-failure`, `on-complete`, `max-retries`)
- Optional timeout

The `state` module tracks `WorkflowRun` with `WorkflowStatus` (Running, Completed, Failed, Stuck) and persists runs to disk. The `run_loop` orchestrates step execution by loading/persisting state and delegating to a `StepExecutor` trait.

### 6. Theme System

Themes are **embedded constants** (currently only Dracula). The `theme::Theme` struct defines ~20 style slots (panels, lists, preview, status bar, modals, indicators). The TUI converts these to `ratatui::Style` at render time via `theme_style_to_style()`.

### 7. Session Naming Convention

Sessions are named `{project}:{branch}`. Branch names containing `/` are normalized to `-` via `sanitize_branch_name()` so they are Zellij-safe. Worktrees and sessions share this naming convention for cross-referencing.

### 8. DepCheck Guard

Every CLI invocation runs `check_deps()` which verifies `zellij >= 0.44.0`, `wt >= 0.34.0`, and `gh >= 2.0.0` are installed. The binary exits with an error message listing all failures before any command runs.

---

## Data & Control Flow

### 1. Startup → Subcommand Dispatch

```
main()
  └─ check_deps()                 — verify zellij/wt/gh available
  └─ run()
       ├─ No args          → cmd_tui()      — interactive TUI
       ├─ "list"           → cmd_list()     — list sessions
       ├─ "open"           → cmd_open()     — attach or create session
       ├─ "close"          → cmd_close()    — detach from session
       ├─ "delete"         → cmd_delete()   — kill session + optional worktree
       ├─ "prune"          → cmd_prune()    — clean orphaned sessions/worktrees
       ├─ "notify"         → cmd_notify()   — write notification + dispatch
       ├─ "autopilot"      → cmd_autopilot_dispatch()
       ├─ "logs"           → cmd_logs()     — print log entries
       ├─ "switch"         → cmd_switch()   — session switcher TUI
       ├─ "logs-viewer"    → cmd_logs_viewer() — scrolling log viewer TUI
       └─ "actions"        → cmd_actions()  — action picker TUI
```

### 2. TUI Event Loop (cmd_tui / run_tui)

```
run_tui()
  └─ enable_raw_mode() + EnterAlternateScreen
  └─ TuiState::with_preview_source()
  └─ event_loop()
       ├─ poll_preview()          — check async git data
       ├─ poll_forge()            — check async PR/CI/Zellij data
       ├─ poll_gh()               — check async gh issue/PR list
       ├─ poll_refresh()          — check async session refresh
       ├─ trigger_refresh()       — spawn refresh if interval elapsed
       ├─ terminal.draw(render)   — ratatui frame render
       │    ├─ render_projects()  — left panel (30%)
       │    ├─ render_sessions()  — right panel (70%)
       │    ├─ render_preview()   — middle strip (8 lines)
       │    ├─ render_status()    — bottom strip (4 lines)
       │    └─ render_modal()     — overlay on top
       └─ event::poll(100ms) → dispatch key
            ├─ Modal mode  → advance_modal() → apply_* / return action
            ├─ Search mode → update query → filter projects/sessions
            └─ Normal mode → navigation + action keys
                 ├─ o/Enter → TuiAction::Open (leaves TUI)
                 ├─ n       → TuiAction::New (branch input → open)
                 ├─ a       → workflow selector modal (→ TuiAction::RunWorkflow)
                 ├─ r / Alt+z r → action menu modal (→ TuiAction::RunAction)
                 ├─ A       → AddProject modal (in-place via TuiCallbacks)
                 ├─ E/D     → EditProject / DeleteProject modal
                 ├─ d/X     → kill session modal (in-place)
                 ├─ p/P     → prune (in-place via callback)
                 ├─ e       → TuiAction::EditPerRepoConfig
                 ├─ /       → search mode
                 └─ K/J     → reorder projects (in-place via callback)
  └─ leave alternate screen
  └─ match action → cmd_open / cmd_autopilot_run / loop back to run_tui
```

### 3. Session Open Flow (cmd_open)

```
cmd_open(project, branch, prompt?)
  └─ KdlProjectStore.get_project(name)
  └─ Remote? → cmd_open_remote()
  │    └─ ssh/mosh <host> "cd <path> && zigzag open <project> <branch>"
  │
  └─ Local flow:
       ├─ SessionManager.list_sessions(project)
       ├─ plan_open_session() → OpenSessionPlan
       ├─ existing session? → SessionManager.attach_session()
       └─ create new:
            ├─ WorktreeManager.list_worktrees()
            ├─ existing worktree? → reuse path | create via `wt switch -c <branch>`
            ├─ effective_layout(global, per_repo) → merge layout config
            ├─ inject_prompt_into_layout() for issue/PR templates
            ├─ inject_claude_stop_hook() → .claude/settings.json
            └─ SessionManager.create_session() → Zellij with KDL layout
```

### 4. Autopilot Workflow Execution

```
cmd_autopilot_run(project, workflow_name)
  └─ load workflow from builtin OR per-repo config
  └─ load_or_start_run() → WorkflowRun (resume or fresh)
  └─ execute_workflow_run() loop:
       ├─ execute_current_step(workflow, run, step_executor)
       │    ├─ run command / send notification / prompt confirmation
       │    └─ return StepResult
       ├─ advance_run(workflow, run, result)
       │    └─ state::advance() → next_step
       ├─ persist → save_run()
       ├─ notify → event_from_advance() → dispatch
       └─ exit on: step limit / terminal state / workflow completion
```

### 5. Preview Data Acquisition

```
TuiState.trigger_preview_load()
  └─ Thread 1 (fast): load_git_preview()
  │    ├─ resolve_worktree_path() — find actual worktree dir
  │    ├─ Local:  git rev-parse / status / log via subprocess
  │    └─ Remote: ssh <host> git commands
  │
  └─ Thread 2 (slow): load_extra_preview()
       ├─ ForgeClient.get_pr() → gh pr view --json
       ├─ ForgeClient.get_ci_status() → gh pr view --json statusCheckRollup
       ├─ ForgeClient.get_review_status() → gh pr view --json reviews
       ├─ Zellij: parse_zellij_session_info() → tab/pane/uptime
       └─ Merge into PreviewData::Ready via poll_forge()
```

---

## Integration Points

### External CLI Dependencies
| Tool | Min Version | Usage |
|------|-------------|-------|
| `zellij` | >= 0.44.0 | Session management, layout generation, keybind injection |
| `wt` (worktrunk) | >= 0.34.0 | Git worktree creation/removal |
| `gh` (GitHub CLI) | >= 2.0.0 | PR/CI/review queries, issue listing |

### File System Integration
| Path | Purpose |
|------|---------|
| `~/.config/zigzag/config.kdl` | Global configuration |
| `~/.config/zigzag/projects.kdl` | Project registry |
| `~/.config/zigzag/worktree-metadata.json` | Worktree-first metadata, pending notifications, and agent status |
| `<project>/.config/zigzag.kdl` | Per-repo configuration |
| `/tmp/zigzag/logs/` | Structured log files |
| `/tmp/zigzag/activity.kdl` | Session attach timestamps |
| `/tmp/zigzag/autopilot/` | Workflow run state persistence |
| `<project>/.claude/settings.json` | Claude Code stop hook injection |

### Remote Machine Integration
- **SSH/Mosh transport** for opening/killing sessions on remote hosts
- Remote commands wrapped in `bash -l -c` for login shell profile loading
- Remote git preview via SSH-piped `git` commands
- `mosh` support for iOS clients (no shell_quoting — mosh handles it internally)

### Notification Channels
| Channel | Trigger | Implementation |
|---------|---------|----------------|
| Metadata | `zigzag notify` / `zigzag notify --event` | Write notification records to worktree metadata |
| macOS native | `notifications.macos-native true` | `osascript` System Events |
| Telegram | `notifications.telegram true` + token/chat_id | `curl` to Telegram Bot API |
| TUI badges | Metadata channel (polled every 5s) | 🔔 indicator in sessions list |

### Forge (GitHub) Integration
- `gh` CLI invoked for PR/CI/review data
- JSON output parsed via zigzag-core's `gh` module (no API dependency — pure JSON from `gh` subprocess)
- Issue/PR title slugs used for branch naming convention `grill/{number}-{slug}`

### AI/Review Tool Integration
- Action menu includes `codex` (default) or configurable review tool
- Prompt templates for issue-based and PR-based sessions (`/grill-me ...`)
- Claude Code `zigzag notify` hook injected into `.claude/settings.json` on session creation
- Autopilot workflows invoke `claude` for CI-fix and review-resolution commands

### Zellij Layout Generation
- Keybindings injected for `Alt+k` (switch), `Alt+l` (logs), `Alt+z` (actions)
- Theme colors converted to Zellij KDL `colors { ... }` blocks
- Session name injected as `ZIGZAG_SESSION_NAME` env var for child processes
- Default UI chrome (tab-bar, status-bar) included in all generated layouts
