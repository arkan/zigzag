# z/crates/z-core/src/ — Domain Logic & Pure Abstractions

## Responsibility

`z-core` is the shared library crate that defines the domain model, configuration
parsing, action system, forge integration helpers, theme engine, and all trait
abstractions for the `z` CLI and TUI. It has zero executable surface —
every binary target (`z-cli`, `z-tui`) depends on `z-core` for its core types,
parsing, and rendering primitives.

This crate is deliberately **I/O-agnostic at the boundary**: traits (e.g.
`ProjectStore`, `SessionManager`, `DepChecker`, `NotificationStore`,
`ActivityStore`) define storage and execution interfaces; concrete adapters
live in consumer crates.

## Design Patterns

| Pattern | Application |
|---|---|
| **Trait-based abstraction** | `ProjectStore`, `SessionManager`, `WorktreeManager`, `ForgeClient`, `Notifier`, `DepChecker`, `Logger`, `ActivityStore`, `SessionRefresher`, `WorktreeMetadataStore`, `ConfigEnvironment` — all define pure interfaces injected by the CLI/TUI adapters. |
| **Trait-object polymorphism** | `dyn ActivityStore` used in `session_entry.rs` for best-effort effect composition. |
| **Companion function + trait** | `depcheck::check_deps()` takes a `&impl DepChecker`; `config::parse_projects_kdl_with_environment()` takes `&impl ConfigEnvironment` — enabling deterministic testing without I/O. |
| **Three-tier config merging** | Hardcoded defaults ← global `~/.config/z/config.kdl` ← per-repo `.config/z.kdl`. Used for `layout`, `actions`, and prompt templates. Lower tier wins entirely (no partial merge). |
| **Inner-outer layer merge** | `action::merge_actions()` applies layers sequentially by name (later overrides earlier); `disabled: true` removes the action. Used to compose builtin + global + per-repo actions. |
| **Pure KDL generation** | `layout::generate_layout_kdl()` constructs a Zellij KDL layout string from the `Layout` domain struct. No I/O — pure string manipulation with KDL escaping. |
| **Best-effort effect pattern** | `session_entry::mark_existing_session_entered()` runs notification clearing and activity recording, returning a `SessionEntryEffects` struct that reports per-operation success independently. |
| **Strategy for variable resolution** | `config::resolve_env_token_with_environment()` injects a `ConfigEnvironment` trait to allow env-var resolution without coupling to `std::env`. |
| **Recursive JSON scraping** | `gh::collect_string_fields_into()` walks a `serde_json::Value` tree depth-first, collecting all string values matching a given field name — used to extract timestamps from nested `gh` JSON output. |
| **Constructor with validation** | `Session::new()` normalises the branch name via `sanitize_branch_name()` (replacing `/` → `-`) and produces the canonical session name format `project:branch`. |

## Data & Control Flow

```
┌──────────────────────────────────────────────────────────────────────┐
│                          z-core Crate                                │
│                                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────────────┐   │
│  │ domain   │  │ error    │  │ traits   │  │ config            │   │
│  │ (types)  │  │ (ZError) │  │(abstr.)  │  │ (KDL parsing)     │   │
│  └────┬─────┘  └──────────┘  └────┬─────┘  └──┬───────┬───────┘   │
│       │                            │           │       │           │
│       ▼                            ▼           ▼       ▼           │
│  ┌──────────────────────────────────────────────────────┐          │
│  │ action  ──── merge_actions → resolve_actions         │          │
│  │ activity ──── sort_sessions_by_recent_attach         │          │
│  │ gh ────────── parse_gh_issues, parse_pr_view_json    │          │
│  │ zellij ────── parse_zellij_session_info              │          │
│  │ layout ────── generate_layout_kdl, inject_prompt     │          │
│  │ web ───────── dashboard_groups, zellij_session_url   │          │
│  │ template ──── resolve_template                       │          │
│  │ theme ─────── Theme::from_name → to_zellij_kdl       │          │
│  └──────────────────────────────────────────────────────┘          │
│                                                                      │
 │  ┌────────────────┐  ┌──────────────────────┐                       │
 │  │ session_entry  │  │ claude_hook          │                       │
 │  │(effects comp.) │  │ (merge_stop_hook)    │                       │
 │  └────────────────┘  └──────────────────────┘                       │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │ depcheck — DepChecker trait + check_deps()                  │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │ log — LogEntry format/parse + Logger trait                  │    │
│  └─────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

**Config bootstrap flow:**
1. `config::parse_projects_kdl()` → `Vec<Project>`
2. `config::parse_global_config_kdl()` → `GlobalConfig` (layout, actions, theme, deps, notifications, prompts)
3. `config::parse_per_repo_config_kdl()` → `PerRepoConfig` (layout override, deploy command, autopilot, actions, prompts)
4. `config::effective_layout()`: per-repo > global > default
5. `config::effective_issue_prompt_template()` / `effective_pr_prompt_template()`: per-repo > global > hardcoded

**Action pipeline:**
1. KDL `action { ... }` nodes → `parse_actions_kdl()` → `Vec<ActionDef>`
2. `merge_actions(&[builtin, global_config, per_repo_config])` → deduped list
3. With runtime `ActionEnv` (project, branch, PR data, CI status, comments): `resolve_actions()` → `Vec<ResolvedAction>`
4. Resolve applies: context filter (Session→requires branch), condition eval (`HasPr`, `HasCiFailure`, `HasNewComments`), and `${...}` interpolation.

**Layout generation flow:**
1. `Layout` domain struct → `generate_layout_kdl(layout, bin_path, theme)` → Zellij KDL string
2. Always prepends `default_tab_template { tab-bar + status-bar + children }` for UI chrome
3. Appends tab definitions, `env { Z_SESSION_NAME "..." }` block (if set), `keybinds` block (Alt+k/l/z), and `themes { ... }` block
4. `inject_prompt_into_layout()` appends a prompt string as an argument to the first `command="claude"` pane

**Forge data flow:**
1. `gh` CLI JSON → `gh::parse_gh_issues()` / `parse_gh_prs()` → `Vec<GhItem>`
2. `gh pr view --json number,state,title,url` → `parse_pr_view_json()` → `PullRequest`
3. `gh run list --json conclusion,status` → `parse_ci_status_json()` → `CiStatus`
4. `gh pr view --json reviews,latestReviews,commits` → `parse_review_status_json()` → `ReviewStatus`
5. These feed into `ActionPreview::from_forge_data()` → `ActionEnv`

**Session entry flow:**
1. `session_entry::record_session_attach(activity, session_name)` → `SessionEntryEffects`
2. Best-effort: records attach timestamp; notification clearing is done directly via the metadata store
3. Metadata notification clearing and activity recording are independent operations

## Integration Points

| Interface | Consumer(s) | Description |
|---|---|---|
| `ProjectStore` | `z-cli`, `z-tui` | CRUD for persisted projects |
| `ProjectStoreWriter` | `z-cli`, `z-tui` | Write-side project operations |
| `SessionManager` | `z-cli`, `z-tui` | Zellij session lifecycle (list, create, attach, detach, kill) |
| `WorktreeManager` | `z-cli` | `wt` worktree create/list/remove |
| `ForgeClient` | `z-cli`, `z-tui` | GitHub PR, CI, review queries (backed by `gh` CLI) |
| `Notifier` | `z-cli`, `z-tui` | System notification dispatch (macOS, Telegram, TUI) |
| `SessionRefresher` | `z-tui` | Periodic background fetch of all sessions + notifications + activity |
| `DepChecker` | `z-cli` | Version probing for external tool dependencies (`zellij`, `wt`, `gh`) |
| `Logger` | `z-cli`, `z-tui` | Appending structured log entries |
| `ActivityStore` | `z-cli`, `z-tui` | File-backed last-attach timestamp tracking |
| `WorktreeMetadataStore` | `z-cli`, `z-tui` | JSON metadata persistence for worktree records, notifications, and LLM status |
| `ConfigEnvironment` | `config` | `env:VAR` resolution strategy (injected for tests) |

**Config files consumed:**
- `~/.config/z/config.kdl` — parsed by `parse_global_config_kdl()`
- `~/.config/z/projects.kdl` — parsed by `parse_projects_kdl()`
- `<project-root>/.config/z.kdl` — parsed by `parse_per_repo_config_kdl()`

**External CLI output parsed:**
- `zellij list-sessions --json` — parsed by `zellij::parse_zellij_session_info()`
- `gh issue list --json ...` — parsed by `gh::parse_gh_issues()`
- `gh pr list --json ...` — parsed by `gh::parse_gh_prs()`
- `gh pr view --json ...` — parsed by `gh::parse_pr_view_json()` / `parse_review_status_json()`
- `gh run list --json ...` — parsed by `gh::parse_ci_status_json()`
- `zellij --version` / `wt --version` / `gh --version` — parsed by `depcheck::parse_version()`

**Generated output consumed by Zellij:**
- `generate_layout_kdl()` → Zellij KDL layout string (passed to `zellij --layout`)
- `theme.to_zellij_kdl()` → Zellij `themes { ... }` block appended to layout

**Generated output consumed by Claude Code:**
- `claude_hook::merge_stop_hook()` → `Value` serialized to `.claude/settings.json`
