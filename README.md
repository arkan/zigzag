# z

TUI/CLI project manager for [Zellij](https://github.com/zellij-org/zellij) — orchestrates sessions, git worktrees, and autopilot workflows.

```
z                          # interactive TUI
z open myapp feat/login    # create worktree + Zellij session + launch Claude
z list                     # show all projects and active sessions
z autopilot watch          # observe running autopilot workflows
```

## What it does

- **Project dashboard**: list all your projects, active sessions, branch status, PR/CI state in one TUI
- **Session lifecycle**: `z open` creates a worktree (via [worktrunk](https://github.com/max-sixty/worktrunk)), launches a Zellij session with Claude + shell tabs
- **Autopilot workflows**: define KDL workflows that monitor CI, auto-fix with Claude, merge PRs, deploy — all in the background
- **Local + remote**: transparent access to remote machines via `zellij attach https://...`

## Status

**Early development** — see [docs/PRD.md](docs/PRD.md) and [docs/SPECS.md](docs/SPECS.md) for the full design.

## Dependencies

| Tool | Min version | Role |
|------|-------------|------|
| [zellij](https://github.com/zellij-org/zellij) | 0.44.0 | Terminal multiplexer |
| [worktrunk](https://github.com/max-sixty/worktrunk) (`wt`) | 0.34.0 | Git worktree management |
| [gh](https://cli.github.com/) | 2.0.0 | GitHub CLI |

## Docs

- [PRD](docs/PRD.md) — problem, solution, user stories, decisions
- [Specs](docs/SPECS.md) — architecture, config format, TUI design, autopilot DSL, phasing
- [Sandcastle](.sandcastle/README.md) — AI agent orchestration (parallel issue solving via Claude Code in Docker)

## License

TBD
