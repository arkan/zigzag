# Zigzag

**Rust TUI/CLI for [Zellij](https://github.com/zellij-org/zellij)-based development** — manage projects, git worktrees, Zellij sessions, and workflow automations from one terminal dashboard.

[Install](#install) · [Basic start](#basic-start) · [Shortcuts](#shortcuts) · [Configuration](#configuration) · [Autopilot](#autopilot) · [Development](#development)

## Overview

`zigzag` is for developers who keep many repositories, branches, and terminal sessions open at once. It gives you one workflow around Zellij and git worktrees:

- **Project dashboard** — browse configured projects, worktrees, running sessions, notifications, and git/PR/CI preview data.
- **Worktree-first sessions** — `zigzag open` restores the primary checkout or creates/restores a branch worktree through [`wt`](https://github.com/max-sixty/worktrunk).
- **Zellij integration** — generated layouts include shortcuts for session switching, actions, and logs.
- **Action menu** — run contextual tools such as review, lazygit, PR opening, CI fixes, or custom commands.
- **Autopilot workflows** — KDL-defined background workflows for CI fixes, review follow-up, merge/deploy flows, and custom automation.

```text
┌─ Zigzag ──────────────────────────────────────┐
│ PROJECTS             WORKTREES                 │
│ > myapp              main                 ●    │
│   api                feat/login           🔔   │
│                                                │
│ PREVIEW                                        │
│ myapp:feat/login · dirty · PR #42 · CI passing │
│                                                │
│ [o]pen [n]ew [r]un [K]ill [d]el [?]help [q]uit│
└────────────────────────────────────────────────┘
```

> [!NOTE]
> Zigzag is still in early development. For deeper design context, see [`docs/PRD.md`](docs/PRD.md) and [`docs/SPECS.md`](docs/SPECS.md).

## Install

### Prerequisites

| Tool | Minimum | Used for |
|---|---:|---|
| [Rust](https://www.rust-lang.org/tools/install) | stable | Build toolchain |
| [Zellij](https://github.com/zellij-org/zellij) | `0.44.0` | Terminal sessions |
| [`wt`](https://github.com/max-sixty/worktrunk) | `0.34.0` | Git worktrees |
| [GitHub CLI](https://cli.github.com/) | `2.0.0` | PR, CI, issue data |

Zigzag checks these dependencies at startup and reports missing or outdated tools.

### From a release

```bash
# Latest release
curl -fsSL https://raw.githubusercontent.com/arkan/zigzag/main/install.sh | bash

# Specific version
curl -fsSL https://raw.githubusercontent.com/arkan/zigzag/main/install.sh | ZIGZAG_VERSION=v0.7.0 bash

# Custom install directory, default is ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/arkan/zigzag/main/install.sh | ZIGZAG_INSTALL_DIR=/usr/local/bin bash
```

### With Nix

```bash
# Install from the flake
nix profile install github:arkan/zigzag

# Or try it without installing
nix run github:arkan/zigzag -- list
```

The Nix package wraps `zigzag` with the common runtime tools it shells out to: `zellij`, `wt`, `git`, `gh`, `ssh`, and `mosh`.

### Optional short alias

Zigzag does not install a `z` symlink automatically. If you still want the short command, add this to your shell profile:

```bash
alias z=zigzag
```

### Migration from `z`

Zigzag does not read old `z` paths. If you used the previous name, move local files manually:

```bash
mv "$HOME/.config/z" "$HOME/.config/zigzag"
mv "$HOME/.local/state/z" "$HOME/.local/state/zigzag"
```

### From source

```bash
make install

# Or directly
cargo install --path zigzag/crates/zigzag-cli
```

## Basic start

### 1. Register your projects

Create `~/.config/zigzag/projects.kdl`:

```kdl
project "myapp" {
    path "~/Code/myapp"
}

project "api" {
    path "~/Code/api"
}
```

### 2. Open the dashboard

```bash
zigzag
```

Then use:

| Key | Action |
|---|---|
| `↑` / `↓` | Move in the current list |
| `←` / `→` or `Tab` | Switch between Projects and Worktrees |
| `Enter` or `o` | Open/restore the selected worktree session |
| `s` or `Alt+k` | Open the local active-session switcher modal |
| `n` | Create a new worktree + session from a branch, issue, or PR |
| `/` | Fuzzy search |
| `?` | Show in-app help |
| `q` | Quit the dashboard |

> [!TIP]
> Arrow keys always work. `h/j/k/l` navigation is available when `keybindings "vim"` is enabled in your config.

### 3. Open directly from the shell

```bash
zigzag open myapp              # Open/restore the primary checkout session
zigzag open myapp feat/login   # Open/restore a branch worktree + session
zigzag switch                  # Pick another local Zigzag-managed session
```

When a branch worktree does not exist yet, `zigzag open <project> <branch>` creates it with `wt`, generates a Zellij layout, starts the session, then attaches to it.

### 4. Return to Zigzag from Zellij

Inside a session, press `Ctrl+O`, then `D` to detach. The Zellij session keeps running in the background, and you can return with `zigzag` or `zigzag open <project> [branch]`.

## Shortcuts

### Dashboard shortcuts

| Key | Action |
|---|---|
| `o` / `Enter` | Open/restore selected project or worktree |
| `s` / `Alt+k` | Open the local active-session switcher modal |
| `n` | New worktree + session |
| `r` | Run action menu |
| `a` | Autopilot workflows |
| `K` | Kill active session only |
| `d` | Delete selected worktree |
| `D` | Run doctor diagnostics |
| `A` / `E` / `X` | Add, edit, or delete a project |
| `e` | Edit per-repo `.config/zigzag.kdl` |
| `/` | Search |
| `?` | Help |

### In Zigzag-managed Zellij sessions

Zigzag injects these shortcuts into generated Zellij layouts:

| Shortcut | Action |
|---|---|
| `Alt+k` | Open the floating `zigzag switch` session picker |
| `Alt+z` | Open the floating action menu |
| `Alt+l` | Open the floating log viewer |

### Useful Zellij defaults

| Shortcut | Action |
|---|---|
| `Ctrl+O`, then `D` | Detach from the session |
| `Ctrl+Q` | Quit the session |
| `Ctrl+T`, then `N` | New tab |
| `Ctrl+T`, then `1`-`9` | Switch to tab by number |

## Useful commands

| Command | Description |
|---|---|
| `zigzag` | Launch the dashboard |
| `zigzag list` | List configured projects and active sessions |
| `zigzag open <project> [branch]` | Open/restore a checkout or branch session |
| `zigzag close [session]` | Detach a session without deleting it |
| `zigzag switch` | Pick and jump to another local Zigzag session |
| `zigzag actions` | Open the action menu for the current session |
| `zigzag logs [-n <count>]` | Show Zigzag logs |
| `zigzag doctor [--fix]` | Diagnose or repair safe project/session issues |
| `zigzag session kill <project> <branch>` | Kill a Zellij session only |
| `zigzag worktree delete <project> <branch> [--confirm <branch>]` | Delete a worktree |
| `zigzag project delete <project>` | Remove a project from `projects.kdl` |
| `zigzag notify [session] <message>` | Add a session notification |
| `zigzag autopilot <subcommand>` | Manage workflow automation |

## Configuration

Zigzag uses [KDL](https://kdl.dev) with three main files:

| File | Purpose |
|---|---|
| `~/.config/zigzag/projects.kdl` | Project registry |
| `~/.config/zigzag/config.kdl` | Global preferences, layout defaults, actions, notifications |
| `<repo>/.config/zigzag.kdl` | Per-repository layout, actions, and autopilot settings |

Minimal global config:

```kdl
keybindings "vim" // or omit for arrow-key navigation
theme "dracula"

notifications {
    tui true
    macos-native false
}
```

Minimal per-repo layout:

```kdl
layout {
    tab "code" {
        pane "editor"
        pane "shell" size=30
    }

    tab "agent" {
        pane command="claude"
    }
}
```

## Actions and notifications

The action menu (`r` in the dashboard, `Alt+z` in a generated Zellij session) resolves built-in, global, and per-repo actions against the current project/session context.

Notifications are stored in local worktree metadata and surfaced as dashboard/switcher badges. From inside a Zellij pane, `$ZELLIJ_SESSION_NAME` lets you notify the current session without naming it:

```bash
zigzag notify "CI finished" --level info
zigzag notify myapp:feat-login "Review comments arrived" --level warning
```

## Autopilot

Autopilot workflows are KDL-defined state machines that can run commands, send notifications, ask for confirmations, and transition on success/failure.

```bash
zigzag autopilot list
zigzag autopilot status
zigzag autopilot run <project> <workflow>
```

Built-in workflows cover common PR and deploy loops such as CI fixing, review follow-up, merge-when-ready, Dependabot auto-merge, and deploy monitoring.

## Development

This repository is a Rust workspace:

```text
zigzag/crates/
├── zigzag-core       # Domain types, config, actions, traits
├── zigzag-cli        # CLI entry point and process/filesystem adapters
├── zigzag-tui        # Ratatui dashboard, switcher, logs, action picker
├── zigzag-autopilot  # Workflow DSL and runner
├── zigzag-plugin     # Future Zellij WASM plugin
└── zigzag-web        # Future web bridge
```

Common checks:

```bash
npm run typecheck
npm test
cargo fmt --manifest-path zigzag/Cargo.toml --all --check
```

Further reading:

- [`codemap.md`](codemap.md) — repository map and entry points
- [`docs/PRD.md`](docs/PRD.md) — product requirements
- [`docs/SPECS.md`](docs/SPECS.md) — technical specs
