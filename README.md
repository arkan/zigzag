# z

**TUI/CLI project manager for [Zellij](https://github.com/zellij-org/zellij)** — orchestrates sessions, git worktrees, and autopilot workflows from a single dashboard.

[Overview](#overview) | [Getting started](#getting-started) | [Usage](#usage) | [Working with Zellij](#working-with-zellij) | [Notifications](#notifications) | [Configuration](#configuration) | [Autopilot](#autopilot) | [Architecture](#architecture)

## Overview

`z` unifies your development workflow around Zellij. Instead of juggling terminals, branches, and CI dashboards separately, `z` gives you a single TUI to manage everything:

- **Project dashboard** — browse all your projects, active sessions, git branch status, PR/CI state
- **Session lifecycle** — `z open` creates a git worktree (via [worktrunk](https://github.com/max-sixty/worktrunk)), launches a Zellij session with Claude + shell tabs, and attaches
- **Autopilot workflows** — KDL-defined state machines that monitor CI, auto-fix failures with Claude, merge PRs, deploy — running in the background
- **Local + remote** — transparent access to remote machines via Zellij HTTPS attach
- **Fuzzy search** — instantly filter projects and sessions

```
┌─ z ────────────────────────────────────────────┐
│  PROJECTS             SESSIONS                  │
│  > myapp    ●         main                      │
│    hermes   ●         feat/login                │
│    prod-api                                     │
│  ┌─ PREVIEW ─────────────────────────────────┐  │
│  │ myapp:feat/login                           │  │
│  │ branch: 3 ahead, 1 behind · dirty          │  │
│  │ PR: #42 (open) | CI: passing               │  │
│  │ session: 3 tabs, 5 panes, up 2h34m         │  │
│  └───────────────────────────────────────────┘  │
│  [o]pen [n]ew [d]elete [p]rune [a]utopilot [/] │
└─────────────────────────────────────────────────┘
```

> [!NOTE]
> `z` is in **early development**. See the [PRD](docs/PRD.md) and [Specs](docs/SPECS.md) for the full design.

## Getting started

### Prerequisites

| Tool | Min version | Role |
|------|-------------|------|
| [Rust](https://www.rust-lang.org/tools/install) | stable | Build toolchain |
| [Zellij](https://github.com/zellij-org/zellij) | 0.44.0 | Terminal multiplexer |
| [worktrunk](https://github.com/max-sixty/worktrunk) (`wt`) | 0.34.0 | Git worktree management |
| [gh](https://cli.github.com/) | 2.0.0 | GitHub CLI |

`z` checks for these dependencies at startup and reports any missing or outdated tools.

### Install

```bash
# From the repository
make install

# Or directly with cargo
cargo install --path z/crates/z-cli
```

### First-time setup

1. Create your project list at `~/.config/z/projects.kdl`:

```kdl
project "myapp" {
    path "~/Code/myapp"
}

project "api" {
    path "~/Code/api"
}

// Remote project
project "prod" {
    path "/srv/app"
    host "https://remote:8080"
    token "env:ZELLIJ_TOKEN"
}
```

2. Run `z` to open the TUI.

## Usage

### Commands

```bash
z                                # Launch interactive TUI
z list                           # List all projects and active sessions
z open <project> [branch]        # Open/attach a session (creates worktree if needed)
z close [session]                # Detach session (keep worktree)
z delete <project:branch>        # Kill session + prompt to delete worktree
z prune [--dry-run]              # Clean orphaned sessions and worktrees
z switch                         # Switch between sessions (interactive)
z notify [session] <msg> [--level info|warning|error]
z autopilot <subcommand>         # Manage autopilot workflows
z logs [-n <count>]              # View logs
```

### Opening a project

```bash
# Open on the main branch (default)
z open myapp

# Open or create a feature branch worktree
z open myapp feat/login
```

This will:
1. Create a git worktree for the branch (via `wt`) if it doesn't exist
2. Generate a Zellij layout with configured tabs/panes
3. Launch a new Zellij session named `myapp:feat-login`
4. Attach to the session

### TUI keybindings

| Key | Action |
|-----|--------|
| `o` | Open selected project/session |
| `n` | New branch on selected project |
| `d` | Delete session + worktree |
| `p` | Prune orphaned sessions |
| `a` | Open autopilot panel |
| `/` | Fuzzy search |
| `q` | Quit |

Both arrow keys and vim-style (`hjkl`) navigation are supported (configurable).

## Working with Zellij

When you `z open` a project, `z` generates a KDL layout and launches a Zellij session. By default, each session gets a `claude` tab and a `shell` tab (customizable via config).

### Zellij basics

Inside a Zellij session, useful built-in shortcuts:

| Shortcut | Action |
|----------|--------|
| `Ctrl+T` then `N` | New tab |
| `Ctrl+T` then `1-9` | Switch to tab by number |
| `Ctrl+O` then `D` | Detach from session (session keeps running) |
| `Ctrl+Q` | Quit session |

> [!TIP]
> Detaching (`Ctrl+O D`) lets you return to the `z` TUI while your session stays alive in the background. Re-attach anytime with `z open <project>`.

### Session shortcuts

`z` injects custom keybindings into every Zellij session:

| Shortcut | Action |
|----------|--------|
| `Alt+K` | **Switch session** — floating picker to jump between z-managed sessions |
| `Alt+L` | **Logs** — view z logs in a floating pane |
| `Alt+G` | **Lazygit** — open lazygit full-screen in a floating pane |

These shortcuts work in all Zellij modes. Floating panes close automatically on exit.

### Session naming

Sessions are named `{project}:{branch}` with `/` replaced by `-`. For example, opening project `myapp` on branch `feat/login` creates session `myapp:feat-login`.

## Notifications

`z` has a file-based notification system that connects background processes (CI, autopilot, scripts) to the TUI.

### Sending notifications

```bash
# Explicit session
z notify myapp:main "CI passed"

# Infer session from $ZELLIJ_SESSION_NAME (works inside any Zellij pane)
z notify "Deploy complete" --level warning
```

Levels: `info` (default), `warning`, `error`.

### Where notifications appear

- **TUI** — sessions with pending notifications show a 🔔 badge. Notifications are cleared when you open the session.
- **macOS** — native system notifications (enable with `macos-native true` in config)
- **Telegram** — push to a Telegram chat (configure `telegram-token` and `telegram-chat-id` in config)

> [!TIP]
> Inside a Zellij pane, `$ZELLIJ_SESSION_NAME` is set automatically, so `z notify "message"` just works — no need to specify the session.

## Configuration

`z` uses [KDL](https://kdl.dev) for configuration with three tiers (global < project list < per-repo):

### Global config — `~/.config/z/config.kdl`

```kdl
default-layout {
    tab "code" {
        pane "editor"
        pane "shell" size=30
    }
    tab "claude" {
        pane "claude"
    }
}

keybindings "vim"     // or "arrows" (default)

theme "dracula"

notifications {
    macos-native true
    tui true
}
```

### Per-repo config — `.config/z.kdl` (in repo root)

```kdl
layout {
    tab "code" {
        pane "editor"
        pane "shell" size=30
    }
    tab "test" {
        pane command="cargo watch -x test"
    }
}

claude {
    args "--resume"
}

autopilot {
    auto-push true
}
```

## Autopilot

Autopilot workflows are KDL-defined state machines that run in the background, reacting to events and executing actions automatically.

### Built-in workflows

| Workflow | Trigger | Description |
|----------|---------|-------------|
| `pr-ci-fix` | `post-push` | Monitor CI, fix failures with Claude (max 3 retries) |
| `pr-review-fix` | `pr-review-received` | Resolve PR review comments with Claude |
| `pr-merge-when-ready` | `pr-approved` | Wait for CI green, squash-merge, cleanup |
| `dependabot-auto` | `pr-opened-by-dependabot` | Auto-merge Dependabot PRs if tests pass |
| `deploy-watch` | `post-merge-main` | Monitor deploy, rollback on failure |
| `deploy-sync` | `new-commits-on-main` | Pull, diff, confirm, deploy |

### Custom workflows

Define your own in `.config/z.kdl`:

```kdl
autopilot "my-workflow" {
    trigger "manual"
    step "build" {
        run "cargo build --release"
        on-success "test"
        on-failure "notify-fail"
    }
    step "test" {
        run "cargo test"
        on-success "done"
        on-failure "notify-fail"
    }
    step "notify-fail" {
        notify "Build failed!" level="error"
    }
}
```

**Step actions:** `run "command"`, `notify "message"`, `confirm "prompt?"`
**Transitions:** `on-success`, `on-failure`, `on-complete`, `on-max-retries`, `on-accept`, `on-reject`

## Architecture

`z` is a Rust workspace organized into focused crates:

```
z/crates/
├── z-core       # I/O-agnostic business logic + trait definitions
├── z-cli        # CLI commands, binary entry point, trait implementations
├── z-tui        # ratatui TUI frontend
├── z-autopilot  # State machine engine, KDL DSL parser, built-in workflows
├── z-plugin     # (planned) Zellij WASM plugin
└── z-web        # (planned) Web UI via axum + ratatui WASM
```

All I/O is abstracted behind traits in `z-core` (`SessionManager`, `WorktreeManager`, `ForgeClient`, `Notifier`, etc.), making the business logic fully testable without real processes or filesystem access.

### Key dependencies

| Crate | Purpose |
|-------|---------|
| [ratatui](https://ratatui.rs) | TUI rendering |
| [crossterm](https://github.com/crossterm-rs/crossterm) | Terminal I/O |
| [kdl](https://github.com/kdl-org/kdl-rs) | Configuration parsing |

## Docs

- [PRD](docs/PRD.md) — problem, solution, user stories, decisions
- [Specs](docs/SPECS.md) — architecture, config format, TUI design, autopilot DSL, phasing
- [Sandcastle](.sandcastle/README.md) — AI agent orchestration (parallel issue solving via Claude Code in Docker)
