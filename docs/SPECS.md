# Z — Specifications

TUI + CLI project manager built on [Zellij](https://github.com/zellij-org/zellij), written in Rust.

See also: [PRD](./PRD.md)

---

## 1. Vision

`z` unifies dev project management: terminal sessions, git worktrees, CI/CD monitoring, and automation via Claude — all in a beautiful TUI.

```
z (no args)    → interactive TUI
z open <proj>  → direct CLI
z list         → list projects/sessions
z autopilot    → automated workflows
```

---

## 2. External Dependencies

| Tool | Role | Required |
|------|------|----------|
| [zellij](https://github.com/zellij-org/zellij) | Terminal multiplexer, sessions, layouts | yes |
| [worktrunk](https://github.com/max-sixty/worktrunk) (`wt`) | Git worktree management | yes |
| [gh](https://cli.github.com/) | GitHub CLI — PR, CI status | yes |

`z` checks presence and minimum version of each tool at launch. Fails with a clear message if missing.

---

## 3. Architecture

### 3.1 Rust Workspace

```
z/
├── Cargo.toml          # workspace
├── crates/
│   ├── z-core/         # business logic, 100% I/O-agnostic
│   ├── z-tui/          # ratatui frontend
│   ├── z-cli/          # non-interactive commands
│   ├── z-autopilot/    # state machine, workflows, triggers, notifications
│   ├── z-plugin/       # future WASM Zellij plugin (phase 4)
│   └── z-web/          # future web server axum (phase 5)
```

### 3.2 z-core — Fundamental Constraint

`z-core` is **100% I/O-agnostic**. No direct calls to `std::fs`, `std::process::Command`, or any system I/O. Everything goes through traits:

```rust
trait ProjectStore {
    fn list_projects(&self) -> Result<Vec<Project>>;
    fn get_project(&self, name: &str) -> Result<Project>;
}

trait SessionManager {
    fn list_sessions(&self, project: &str) -> Result<Vec<Session>>;
    fn create_session(&self, project: &str, branch: &str, layout: Layout) -> Result<Session>;
    fn attach_session(&self, session: &Session) -> Result<()>;
    fn kill_session(&self, session: &Session) -> Result<()>;
}

trait WorktreeManager {
    fn list_worktrees(&self, project: &str) -> Result<Vec<Worktree>>;
    fn create_worktree(&self, project: &str, branch: &str) -> Result<Worktree>;
    fn remove_worktree(&self, worktree: &Worktree) -> Result<()>;
}

trait ForgeClient {
    fn get_pr(&self, project: &str, branch: &str) -> Result<Option<PullRequest>>;
    fn get_ci_status(&self, project: &str, branch: &str) -> Result<CiStatus>;
}

trait Notifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()>;
}
```

**Reason**: enables compiling z-core to WASM for the Zellij plugin (phase 4) and web client (phase 5).

### 3.3 Single Binary

- `z` (no args) → launches ratatui TUI
- `z <command> [args]` → direct CLI execution
- Behavior like `lazygit` (TUI) vs `git` (CLI)

---

## 4. Configuration

### 4.1 Global Config — `~/.config/z/config.kdl`

```kdl
// Global preferences
config {
    // Default layout for new sessions
    default-layout {
        tab name="claude" {
            pane command="claude" {
                args "--dangerously-skip-permissions"
            }
        }
        tab name="shell" {
            pane
        }
    }

    // TUI keybindings
    keybindings {
        navigation "arrows" // "arrows" (default) or "vim"
    }

    // Notifications
    notifications {
        macos-native true   // default local
        telegram false      // configurable
        tui true            // always in TUI if open
    }

    // Dependencies — minimum versions
    deps {
        zellij ">=0.44.0"
        wt ">=0.34.0"
        gh ">=2.0.0"
    }
}
```

### 4.2 Project Config — `~/.config/z/projects.kdl`

```kdl
project "myapp" {
    path "~/Code/myapp"
}

project "hermes" {
    path "~/Library/Mobile Documents/iCloud~md~obsidian/Documents/HERMES"
    layout "obsidian" // reference a named layout
}

project "prod-api" {
    path "~/Code/prod-api"
    host "https://vps.example.com:8082"
    token "env:ZP_VPS_TOKEN"
}
```

### 4.3 Per-Repo Config — `.config/z.kdl`

```kdl
// Override layout for this project
layout {
    tab name="claude" {
        pane command="claude" {
            args "--resume"
        }
    }
    tab name="shell" {
        pane
    }
    tab name="server" {
        pane command="npm" {
            args "run" "dev"
        }
    }
    tab name="logs" {
        pane command="tail" {
            args "-f" "/var/log/app.log"
        }
    }
}

// Deployment
deploy {
    command "./deploy.sh"
}

// Autopilot overrides
autopilot {
    auto-push true   // default
    review false      // default
}
```

---

## 5. Conventions

### 5.1 Session Naming

Format: `{project}:{branch}`

Examples:
- `myapp:main`
- `myapp:feat-login`
- `prod-api:fix-bug-42`

`/` in branch names is replaced by `-` for Zellij URL compatibility.

### 5.2 Worktrees

Fully managed by worktrunk (`wt`). `z` calls `wt switch`, `wt remove`, `wt list`. No custom worktree logic.

---

## 6. CLI Commands

### 6.1 Session Management

```bash
z list                        # List projects + active sessions
z open <project> [branch]     # Open/attach a session
z close <session>             # Detach session (keep worktree)
z delete <session>            # Kill session + confirm worktree deletion
z prune                       # Clean orphaned sessions
```

### 6.2 `z open` — Workflow

```
z open myapp
  → Local or remote project?
  → LOCAL:
      → Existing session for main? → attach
      → No session? → create session myapp:main with layout
  → Branch choice:
      → main (default)
      → existing branch (existing worktree)
      → new branch → wt switch -c <branch> → create session

z open myapp feat/login
  → Worktree exists? → attach session myapp:feat-login
  → Otherwise → wt switch -c feat/login → create session → launch layout

z open prod-api feat/x    (remote project)
  → ssh vps "cd ~/Code/prod-api && wt switch -c feat/x"
  → zellij attach https://vps.example.com:8082/prod-api:feat-x --token $ZP_VPS_TOKEN
```

### 6.3 `z delete` — Workflow

```
z delete myapp:feat-login
  → Kill Zellij session myapp:feat-login
  → "Delete worktree feat/login? (y/N)"
      → y: wt remove feat/login
      → N: worktree kept
```

---

## 7. TUI

### 7.1 Main Layout

```
┌─ z ──────────────────────────────────────────────────────────────┐
│                                                                   │
│  PROJECTS                       SESSIONS                          │
│  ─────────                      ────────                          │
│  ▸ myapp           ●           main                               │
│    hermes           ●           feat/login        🔔              │
│    prod-api     🌐             feat/dashboard                     │
│                                                                   │
│  ┌─ PREVIEW ──────────────────────────────────────────────────┐  │
│  │ myapp:feat/login                                            │  │
│  │ branch: feat/login (3 ahead, 1 behind) ● dirty             │  │
│  │ PR: #42 (open) | CI: ✅ passing                             │  │
│  │ session: 3 tabs, 5 panes, up 2h34m                          │  │
│  │                                                              │  │
│  │ recent commits:                                              │  │
│  │  a1b2c3 fix: auth token refresh                              │  │
│  │  d4e5f6 feat: login form validation                          │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  [o]pen  [n]ew  [d]elete  [p]rune  [a]utopilot  [/]search  [q]  │
│  myapp | local | worktrees: 3 | autopilots: 1 running            │
└───────────────────────────────────────────────────────────────────┘
```

### 7.2 Indicators

| Icon | Meaning |
|------|---------|
| `●` | Active sessions for this project |
| `🌐` | Remote project |
| `🔔` | Notification (claude finished, CI changed, etc.) |
| `✅` / `❌` | CI passing / failing |
| `● dirty` / `● clean` | Working tree status |

### 7.3 TUI Features

- **Fuzzy search**: `/` to filter projects and sessions
- **Progressive loading**: renders structure immediately, fills preview async (git status, PR, CI)
- **Keybindings**: arrow keys by default, vim-style (`j/k/h/l`) configurable
- **Theme**: auto-detect from terminal

---

## 8. Notifications

### 8.1 Sources

- Claude finished working in a pane
- CI status changed (pass → fail, fail → pass)
- Autopilot completed (success or failure)
- PR review received

### 8.2 Mechanism (phase 1)

File-based: events write to `/tmp/z/notifications/{session}`. The TUI watches this directory.

Phase 4+: migrate to Zellij pipe / plugin events.

### 8.3 Configurable Channels

```kdl
notifications {
    macos-native true    // macOS notification (default local)
    telegram false       // via Telegram bot
    tui true             // in z TUI if open
}
```

---

## 9. Autopilot

### 9.1 Concept

Automated workflows defined in KDL. Execute action sequences in response to triggers, with Claude as a resolution agent.

### 9.2 Execution

- **Background by default** — user can close their laptop
- **Optional pane** — `z autopilot watch` to observe live
- **State persisted to disk** — survives restarts
- **Full auto by default** — Claude commits + pushes directly
- **Configurable**: `auto_push: false` + `review: true` for human-in-the-loop

### 9.3 Built-in Workflows

#### `pr-ci-fix`
```kdl
autopilot "pr-ci-fix" {
    description "Monitor CI, fix failures with Claude, retry"
    trigger "post-push"

    step "monitor-ci" {
        run "gh run watch --exit-status"
        on-failure "fix-ci"
        on-success "notify-done"
    }

    step "fix-ci" {
        run "claude 'Fix the CI failure based on: $(gh run view --log-failed)'"
        max-retries 3
        on-complete "monitor-ci"
        on-max-retries "notify-stuck"
    }

    step "notify-done" {
        notify "PR CI passing ✅"
    }

    step "notify-stuck" {
        notify "PR CI stuck after 3 attempts ❌"
    }
}
```

#### `pr-review-fix`
```kdl
autopilot "pr-review-fix" {
    description "Resolve PR review comments with Claude"
    trigger "pr-review-received"

    step "fix-comments" {
        run "claude 'Resolve all PR review comments: $(gh pr view --json reviews)'"
        on-complete "push-fixes"
    }

    step "push-fixes" {
        run "git push"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "PR review comments resolved ✅"
    }
}
```

#### `pr-merge-when-ready`
```kdl
autopilot "pr-merge-when-ready" {
    description "Auto-merge when PR approved + CI green"
    trigger "pr-approved"

    step "wait-ci" {
        run "gh run watch --exit-status"
        on-success "merge"
        on-failure "notify-ci-fail"
    }

    step "merge" {
        run "gh pr merge --squash --delete-branch"
        on-complete "cleanup"
    }

    step "cleanup" {
        run "z delete {session}"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "PR merged and cleaned up ✅"
    }

    step "notify-ci-fail" {
        notify "PR approved but CI failing ❌"
    }
}
```

#### `dependabot-auto`
```kdl
autopilot "dependabot-auto" {
    description "Auto-merge Dependabot PRs if tests pass"
    trigger "pr-opened-by-dependabot"

    step "run-tests" {
        run "gh run watch --exit-status"
        on-success "merge"
        on-failure "notify-fail"
    }

    step "merge" {
        run "gh pr merge --squash --delete-branch"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "Dependabot PR merged ✅"
    }

    step "notify-fail" {
        notify "Dependabot PR failing ❌ — review needed"
    }
}
```

#### `deploy-watch`
```kdl
autopilot "deploy-watch" {
    description "Monitor deploy after merge, rollback if error"
    trigger "post-merge-main"

    step "monitor-deploy" {
        run "deploy_command --status"
        timeout "10m"
        on-success "notify-done"
        on-failure "rollback"
    }

    step "rollback" {
        run "deploy_command --rollback"
        on-complete "notify-rollback"
    }

    step "notify-done" {
        notify "Deploy successful ✅"
    }

    step "notify-rollback" {
        notify "Deploy failed, rolled back ⚠️"
    }
}
```

#### `deploy-sync`
```kdl
autopilot "deploy-sync" {
    description "Pull main changes, confirm, deploy"
    trigger "new-commits-on-main"
    poll-interval "5m"

    step "pull" {
        run "git pull origin main"
        on-complete "diff-summary"
    }

    step "diff-summary" {
        run "git log --oneline @{1}..HEAD"
        on-complete "confirm-deploy"
    }

    step "confirm-deploy" {
        confirm "Deploy these changes?"
        on-accept "deploy"
        on-reject "notify-skipped"
    }

    step "deploy" {
        run "deploy_command"
        on-success "notify-done"
        on-failure "notify-fail"
    }

    step "notify-done" {
        notify "Deploy successful ✅"
    }

    step "notify-skipped" {
        notify "Deploy skipped by user"
    }

    step "notify-fail" {
        notify "Deploy failed ❌"
    }
}
```

### 9.4 Custom Workflows

Users can define custom workflows in the project's `.config/z.kdl`:

```kdl
autopilot "my-custom-workflow" {
    trigger "manual"

    step "do-stuff" {
        run "./scripts/my-script.sh"
        on-complete "notify"
    }

    step "notify" {
        notify "Done ✅"
    }
}
```

Escape hatch: `run` accepts any shell command.

---

## 10. Remote

### 10.1 Architecture

```
Local machine                     Remote machine
─────────────                    ────────────────
z open prod-api feat/x           zellij (systemd service)
  → ssh vps "wt switch -c feat/x"  → port 8082 HTTPS
  → zellij attach https://...       → auth tokens
```

### 10.2 Remote Machine Prerequisites

- Zellij installed + systemd service active (webserver port 8082)
- worktrunk (`wt`) installed
- Git repos cloned
- SSH access from local machine

### 10.3 Multiplayer

Natively supported by Zellij. Multiple users can attach the same session with distinct colored cursors.

### 10.4 Exited Sessions

Exited (crashed/closed) Zellij sessions are **ignored** in `z list`. No automatic resurrection.

---

## 11. Phasing

| Phase | Scope | Crates |
|-------|-------|--------|
| **1a** | CLI: `z open`, `z list`, `z close`, `z delete`. KDL config. Dep checks. Dynamic layout generation. | z-core, z-cli |
| **1b** | TUI: ratatui, project/session navigation, fuzzy search, basic actions | z-tui |
| **1c** | Enriched TUI: preview pane (git + Zellij + PR/CI), Claude notifications | z-tui, z-core |
| **2** | Cleanup: `z prune`, advanced worktrunk integration | z-core |
| **3** | Remote: SSH setup + Zellij HTTPS attach, host/token config | z-core, z-cli |
| **4** | Zellij WASM plugin — TUI embedded in Zellij | z-plugin |
| **5** | Web UI — ratatui WASM + xterm.js, Leptos fallback + axum | z-web |
| **6** | Autopilot: state machine, KDL DSL, built-in workflows, notifications | z-autopilot |

---

## 12. Decision Log

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | Declarative project config `~/.config/z/projects.kdl` | Explicit control |
| 2 | Session convention `{project}:{branch}` | Readable, unique, URL-compatible |
| 3 | Worktrees via worktrunk (`wt`) | Mature tool |
| 4 | `z delete` = kill session + confirm worktree | Protects unpushed work |
| 5 | Default layout: tab claude + tab shell, override `.config/z.kdl` | Covers 90% of cases |
| 6 | `claude` on every session | Consistency |
| 7 | KDL config everywhere | Coherent with Zellij |
| 8 | Remote: SSH setup + zellij attach HTTPS | Worktree before session |
| 9 | Rust | WASM pipeline |
| 10 | Name `z` | Minimalist |
| 11 | Dep check at launch, fail if missing | Clear UX |
| 12 | Single binary: TUI without args, CLI with args | Simple |
| 13 | z-core 100% I/O-agnostic via traits | WASM portability |
| 14 | TUI ratatui, auto-detect theme | Rust standard |
| 15 | Preview: git + Zellij + PR + CI, progressive loading | Full context |
| 16 | Fuzzy search | Fast navigation |
| 17 | Keybindings arrows default, vim configurable | Accessible |
| 18 | GitHub only via `gh` | Simple |
| 19 | Configurable notifications: macOS native, Telegram, TUI | Flexible |
| 20 | Multiplayer supported | Native Zellij |
| 21 | Exited sessions ignored | Simplicity |
| 22 | Autopilot: KDL DSL + script escape hatch | 80/20 |
| 23 | Autopilot: background default + optional pane | Laptop-closeable |
| 24 | Autopilot: full auto default, configurable human-in-the-loop | Point of autopilot |
| 25 | Web: ratatui WASM + xterm.js, Leptos fallback | Reuses TUI |
| 26 | Deploy via `deploy_command` in project config | Generic |
