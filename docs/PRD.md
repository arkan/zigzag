# Zigzag — Product Requirements Document

See also: [Specs](./SPECS.md)

---

## Problem Statement

Les developpeurs qui travaillent sur plusieurs projets simultanement font face a une fragmentation de leur environnement de travail : sessions terminal eparpillees, worktrees git geres manuellement, aucune visibilite sur l'etat CI/PR depuis le terminal, et aucune automatisation entre ces outils. Passer d'un projet a un autre demande de retrouver la bonne session, naviguer au bon repertoire, verifier manuellement l'etat du build, et relancer les bons outils. Sur des machines distantes, le probleme est encore pire — il faut SSH, retrouver le contexte, et tout reconfigurer.

Il n'existe pas d'outil qui unifie la gestion de projets dev (sessions terminal + worktrees + CI/CD + automatisation) dans une interface unique, que ce soit en local ou a distance.

## Solution

`zigzag` est un gestionnaire de projets dev centre sur Zellij, ecrit en Rust. Il fournit :

- **Un binaire unique** : `zigzag` (TUI interactive) ou `zigzag <command>` (CLI direct)
- **Gestion de projets** : chaque projet declare dans une config KDL est associe a un repo git. Chaque branche de travail correspond a un worktree git (via worktrunk) et une session Zellij dediee
- **Vue unifiee** : une TUI ratatui affiche tous les projets, leurs sessions actives, l'etat git, le statut CI/PR GitHub, et les notifications (ex: Claude a fini de travailler)
- **Automatisation** : un systeme d'autopilot permet de definir des workflows (en KDL) qui reagissent a des evenements (push, CI fail, PR review) et utilisent Claude comme agent de resolution
- **Transparence local/remote** : les projets distants sont accessibles via SSH (setup worktree) + Zellij HTTPS attach, de maniere transparente depuis la meme interface

## User Stories

### Gestion de projets et sessions

1. As a developer, I want to list all my active projects and their sessions in a single view, so that I can see my entire work context at a glance
2. As a developer, I want to open a project session with a single command (`zigzag open myapp`), so that I don't have to manually navigate directories and start tools
3. As a developer, I want `zigzag` to automatically create a Zellij session with Claude and a shell tab when I open a project, so that my environment is ready to work immediately
4. As a developer, I want to open a new branch session (`zigzag open myapp feat/login`), so that a worktree is created and a dedicated Zellij session is launched automatically
5. As a developer, I want to choose between working on `main` or creating a new worktree when opening a project, so that I have full control over my branching strategy
6. As a developer, I want to close a session (`zigzag close`) without deleting the worktree, so that I can resume later
7. As a developer, I want to delete a session (`zigzag delete`) and be prompted to also delete the worktree, so that I can clean up when a feature branch is done
8. As a developer, I want to prune orphaned sessions and worktrees (`zigzag prune`), so that my system stays clean over time
9. As a developer, I want each session named `{project}:{branch}`, so that I can identify them easily and they are compatible with Zellij remote URLs

### Configuration

10. As a developer, I want to declare my projects in a global KDL config (`~/.config/zigzag/projects.kdl`), so that `zigzag` knows where my repos live
11. As a developer, I want to override the default layout per project via `.config/zigzag.kdl` in the repo, so that each project can have custom tabs (server, logs, etc.)
12. As a developer, I want to configure Claude arguments per project (e.g., `--resume`), so that Claude starts with the right context
13. As a developer, I want to set global preferences (keybindings, notifications, default layout) in `~/.config/zigzag/config.kdl`, so that my experience is consistent across projects
14. As a developer, I want to configure minimum dependency versions in my global config, so that `zigzag` warns me if my tools are outdated
15. As a developer, I want `zigzag` to verify that `zellij`, `wt`, and `gh` are installed at launch, so that I get a clear error message instead of cryptic failures

### TUI

16. As a developer, I want an interactive TUI when I run `zigzag` without arguments, so that I can navigate my projects visually
17. As a developer, I want to see a project list on the left and its sessions on the right, so that I can quickly find what I'm looking for
18. As a developer, I want to fuzzy-search projects and sessions by pressing `/`, so that I can navigate large lists quickly
19. As a developer, I want a preview pane showing branch status (ahead/behind, dirty/clean), recent commits, PR number and CI status, and Zellij session info (tabs, panes, uptime), so that I have full context without leaving `zigzag`
20. As a developer, I want the preview to load progressively (structure first, then git, then PR/CI), so that the UI feels responsive even when GitHub API is slow
21. As a developer, I want to see icons indicating active sessions, remote projects, and notifications, so that I can scan the project list quickly
22. As a developer, I want arrow key navigation by default with configurable vim-style keybindings, so that the TUI is accessible but customizable
23. As a developer, I want the TUI theme to auto-detect from my terminal, so that it looks good without manual configuration
24. As a developer, I want keyboard shortcuts for all actions (open, new, delete, prune, autopilot, search, quit), so that I never need a mouse

### Notifications

25. As a developer, I want to be notified when Claude finishes working in a pane, so that I can review the results without polling
26. As a developer, I want to be notified when CI status changes (pass to fail or fail to pass), so that I can react quickly
27. As a developer, I want to be notified when an autopilot completes (success or failure), so that I know the outcome without watching it
28. As a developer, I want to be notified when a PR review is received, so that I can address comments promptly
29. As a developer, I want to configure notification channels (macOS native, Telegram, TUI badge), so that I receive alerts where I want them
30. As a developer, I want notifications to appear as badges in the TUI session list, so that I can see at a glance which sessions need attention

### Autopilot

31. As a developer, I want to define automated workflows in KDL that trigger on events (push, CI fail, PR review), so that repetitive tasks are handled without my intervention
32. As a developer, I want a `pr-ci-fix` autopilot that monitors CI, asks Claude to fix failures, pushes, and retries up to N times, so that CI issues are resolved automatically
33. As a developer, I want a `pr-review-fix` autopilot that reads PR review comments and asks Claude to resolve them, so that I don't have to manually address each comment
34. As a developer, I want a `pr-merge-when-ready` autopilot that merges my PR when it's approved and CI is green, then cleans up the session and worktree, so that the merge lifecycle is fully automated
35. As a developer, I want a `dependabot-auto` autopilot that auto-merges Dependabot PRs when tests pass, so that dependency updates don't pile up
36. As a developer, I want a `deploy-watch` autopilot that monitors deploys and rolls back on failure, so that I can deploy with confidence
37. As a developer, I want a `deploy-sync` autopilot that polls main for new commits, shows a diff summary, asks for confirmation, then deploys, so that I can keep environments in sync with minimal effort
38. As a developer, I want autopilots to run in the background by default so I can close my laptop, and optionally watch them in a Zellij pane (`zigzag autopilot watch`), so that I have full flexibility
39. As a developer, I want autopilot state persisted to disk so it survives restarts, so that long-running workflows are reliable
40. As a developer, I want autopilots to be full-auto by default (Claude commits and pushes directly), so that the automation is truly hands-off
41. As a developer, I want to configure `auto_push: false` and `review: true` per project or per workflow, so that I can require human approval before pushes when needed
42. As a developer, I want to define custom autopilot workflows in my project's `.config/zigzag.kdl`, so that I can automate project-specific tasks
43. As a developer, I want the `run` step in autopilots to accept any shell command, so that I have an escape hatch for arbitrary automation
44. As a developer, I want autopilots to have a `max-retries` setting with a notification when exhausted, so that infinite loops are prevented

### Remote

45. As a developer, I want to declare a remote project with `host` and `token` in my projects config, so that `zigzag` knows how to reach it
46. As a developer, I want `zigzag open` on a remote project to SSH-setup the worktree then `zellij attach https://...`, so that remote sessions are as easy as local ones
47. As a developer, I want to store remote tokens as environment variable references (`env:VAR`), so that secrets are not in plaintext config files
48. As a developer, I want multiplayer support when multiple people attach to the same remote session, so that we can pair-program with distinct cursors
49. As a developer, I want the TUI to show remote projects with a distinct icon, so that I can tell local from remote at a glance

### Future platforms

50. As a developer, I want the core logic in `zigzag-core` to be I/O-agnostic via traits, so that it can be compiled to WASM for a Zellij plugin or web UI without rewriting business logic
51. As a developer, I want a future Zellij WASM plugin that embeds the `zigzag` TUI directly inside Zellij, so that I don't need a separate terminal to manage projects
52. As a developer, I want a future `zigzag web` command that serves the TUI in a browser via xterm.js, so that I can manage projects from any device

## Implementation Decisions

### Architecture

- **Rust workspace** with 6 crates: `zigzag-core` (business logic), `zigzag-tui` (ratatui), `zigzag-cli` (non-interactive commands), `zigzag-autopilot` (state machine + workflows), `zigzag-plugin` (future WASM), `zigzag-web` (future axum + xterm.js)
- **Single binary**: `zigzag` without args launches TUI, `zigzag <cmd>` runs CLI
- **`zigzag-core` is 100% I/O-agnostic** via traits (`ProjectStore`, `SessionManager`, `WorktreeManager`, `ForgeClient`, `Notifier`). No direct `std::fs` or `std::process::Command`. This is the key architectural constraint enabling WASM portability

### External dependencies

- **Zellij** (>= 0.44.0): terminal multiplexer, sessions, layouts, remote HTTPS attach
- **worktrunk** (`wt`) (>= 0.34.0): git worktree management, hooks, cleanup, pruning
- **gh CLI** (>= 2.0.0): GitHub PR and CI status queries
- All three are verified at launch; `zigzag` fails with a clear message if any is missing or below minimum version

### Configuration format

- **KDL everywhere**: global config, project list, per-repo overrides, autopilot workflows. Chosen for coherence with Zellij's native config format
- Three-tier config: global (`~/.config/zigzag/config.kdl`) < project list (`~/.config/zigzag/projects.kdl`) < per-repo (`.config/zigzag.kdl`)

### Session conventions

- Session naming: `{project}:{branch}` with `/` replaced by `-` for URL compatibility
- Default layout per session: tab "claude" (runs `claude`) + tab "shell" (empty terminal), overridable per project
- Claude launched on every session (main and feature worktrees)

### Worktree management

- Fully delegated to worktrunk (`wt switch`, `wt remove`, `wt list`). No custom worktree logic in `zigzag`
- On `zigzag delete`: kill Zellij session, then prompt user to confirm worktree removal via `wt remove`

### Remote architecture

- SSH for worktree setup on remote machine, then `zellij attach https://host:port/session --token`
- Requires Zellij service (systemd), worktrunk, and git repos on the remote machine
- Multiplayer natively supported by Zellij

### TUI

- **ratatui** framework, theme auto-detected from terminal
- Three-panel layout: projects (left), sessions (right), preview (bottom)
- Preview shows: branch tracking, dirty/clean, PR number, CI status, Zellij session info (tabs, panes, uptime), recent commits
- Progressive async loading: structure renders instantly, data fills in as it arrives
- Fuzzy search via `/` key
- Arrow keys by default, vim-style configurable

### Notifications

- Phase 1: file-based (`/tmp/zigzag/notifications/{session}`). Claude Code hook writes to this directory. TUI watches it
- Phase 4+: migrate to Zellij pipe / plugin events
- Channels: macOS native (default local), Telegram, TUI badge. Configurable in global config

### Autopilot

- DSL in KDL for declarative workflow definition, with `run` escape hatch for arbitrary shell commands
- Background execution by default, optional live pane via `zigzag autopilot watch`
- State machine persisted to disk (survives restarts)
- Full-auto by default (Claude commits + pushes). Configurable `auto_push: false` + `review: true` for human-in-the-loop
- 6 built-in workflows: `pr-ci-fix`, `pr-review-fix`, `pr-merge-when-ready`, `dependabot-auto`, `deploy-watch`, `deploy-sync`
- Custom workflows in per-repo `.config/zigzag.kdl`

### Phasing

| Phase | Scope |
|-------|-------|
| 1a | CLI: `zigzag open`, `zigzag list`, `zigzag close`, `zigzag delete`. Config KDL. Dep checks. Dynamic layout generation |
| 1b | TUI: ratatui, project/session navigation, fuzzy search, basic actions |
| 1c | Enriched TUI: preview pane (git + Zellij + PR/CI), Claude notifications |
| 2 | Cleanup: `zigzag prune`, advanced worktrunk integration |
| 3 | Remote: SSH setup + Zellij HTTPS attach, host/token config |
| 4 | Zellij WASM plugin — TUI embedded in Zellij |
| 5 | Web UI — ratatui WASM + xterm.js, Leptos fallback, axum server |
| 6 | Autopilot: state machine, KDL DSL, built-in workflows, notifications |

## Testing Decisions

### What makes a good test

- Test **external behavior through the trait interfaces**, not implementation details
- A test should answer: "does `zigzag-core` produce the correct output given these inputs?" — not "does it call `git status` with the right flags?"
- Use mock implementations of the I/O traits to test `zigzag-core` in isolation, without filesystem or process dependencies
- Integration tests (with real git repos, real `wt`, real `zellij`) live in a separate test suite and run in CI only

### Modules to test

#### `zigzag-core` (priority: high)

- **ProjectStore**: loading/parsing KDL config, project resolution, config merging (global < project < per-repo)
- **SessionManager**: session naming conventions, session lifecycle (create → attach → close → delete), worktree ↔ session mapping
- **WorktreeManager**: worktree creation/deletion flows, interaction with session lifecycle (delete session → prompt worktree removal)
- **ForgeClient**: PR resolution by branch, CI status parsing, error handling when `gh` is unavailable
- **Layout generation**: dynamic KDL layout generation from config (default layout, per-project overrides, Claude args injection)
- **Config parsing**: KDL config validation, three-tier merging, `env:VAR` token resolution, version constraint parsing

#### `zigzag-autopilot` (priority: high)

- **State machine**: step transitions (on-success, on-failure, on-complete, on-max-retries), persistence/recovery from disk, max-retries enforcement
- **Workflow parsing**: KDL autopilot definition parsing, validation (no orphan steps, no cycles, valid triggers)
- **Trigger system**: event matching (post-push, pr-approved, manual, etc.)
- **Notification dispatch**: correct channel selection based on config, message formatting

#### `zigzag-cli` (priority: medium)

- Integration tests: end-to-end flows (`zigzag open` → session created → `zigzag delete` → session killed + worktree prompt)
- Dependency verification: correct error messages when tools are missing or below version

#### `zigzag-tui` (priority: low)

- Snapshot tests for rendered UI states (project list, preview pane, notification badges) using ratatui's test backend
- No interactive testing — trust ratatui's own test infrastructure

## Out of Scope

- **Multi-forge support** (GitLab, Bitbucket, etc.) — GitHub only via `gh` CLI
- **Session resurrection** — exited Zellij sessions are ignored, not resurrected
- **Custom worktree management** — fully delegated to worktrunk, no reimplementation
- **Automatic installation of dependencies** — `zigzag` verifies presence and version, user installs manually
- **IDE integration** — `zigzag` is terminal-native, no VS Code or JetBrains plugins planned
- **Mobile app** — web UI (phase 5) covers mobile access via browser
- **Conflict resolution in worktrees** — delegated to git/developer, not `zigzag`'s responsibility

## Further Notes

- Users who still want the former short command can define `alias z=zigzag` in their shell profile.
- The I/O-agnostic `zigzag-core` constraint adds initial development cost but is non-negotiable — it is the foundation enabling phases 4 (WASM plugin) and 5 (web UI)
- worktrunk already has Claude integration (`wt config plugins claude`) and lifecycle hooks which may simplify some autopilot triggers
- Zellij v0.44+ HTTPS remote attach is recent (March 2026) — early adopter risk is accepted
- The autopilot system is phase 6 but its architecture (state machine, notifications) should be considered from phase 1 to avoid `zigzag-core` redesign later
