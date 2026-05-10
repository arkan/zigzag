# z/crates/z-core/ — Domain Logic & Pure Abstractions

## Responsibility

`z-core` is the I/O-agnostic shared library crate defining the domain model,
configuration parsing, action system, forge data parsing, theme engine, prompt
templates, dependency checking, and all trait abstractions consumed by `z-cli`
and `z-tui`. It has zero executable surface — every binary target depends on
`z-core` for its core types, parsing, and rendering primitives.

**Key constraint**: This crate must never perform I/O (no filesystem, network,
or process execution). All I/O is deferred to consumer crates via traits.

## Design Patterns

| Pattern | Application |
|---|---|
| **Trait-based abstraction** | `ProjectStore`, `ProjectStoreWriter`, `SessionManager`, `WorktreeManager`, `ForgeClient`, `Notifier`, `SessionRefresher`, `DepChecker`, `Logger`, `NotificationStore`, `ActivityStore`, `ConfigEnvironment` — all define pure interfaces injected by CLI/TUI adapters. |
| **Trait-object polymorphism** | `dyn NotificationStore`, `dyn ActivityStore` in `session_entry.rs` for best-effort effect composition. |
| **Companion function + trait** | `depcheck::check_deps()` takes `&impl DepChecker`; `config::parse_projects_kdl_with_environment()` takes `&impl ConfigEnvironment` — enabling deterministic testing without I/O. |
| **Three-tier config merging** | Hardcoded defaults ← global `~/.config/z/config.kdl` ← per-repo `.config/z.kdl`. Lower tier wins entirely (no partial merge). Used for `layout`, `actions`, and prompt templates. |
| **Inner-outer layer merge** | `action::merge_actions()` applies layers sequentially by name (later overrides earlier); `disabled: true` removes the action. Used to compose builtin + global + per-repo actions. |
| **Pure KDL generation** | `layout::generate_layout_kdl()` constructs a Zellij KDL layout string from the `Layout` domain struct. No I/O — pure string manipulation with KDL escaping. |
| **Best-effort effect pattern** | `session_entry::mark_existing_session_entered()` runs notification clearing and activity recording independently, returning a `SessionEntryEffects` struct that reports per-operation success. |
| **Strategy for variable resolution** | `config::resolve_env_token_with_environment()` injects a `ConfigEnvironment` trait to allow env-var resolution without coupling to `std::env`. |
| **Recursive JSON scraping** | `gh::collect_string_fields_into()` walks a `serde_json::Value` tree depth-first, collecting all string values matching a given field name — used to extract timestamps from nested `gh` JSON output. |
| **Constructor with validation** | `Session::new()` normalises the branch name via `sanitize_branch_name()` (replacing `/` → `-`) and produces the canonical session name format `project:branch`. |
| **Embedded theme constants** | `Theme::from_name()` builds a `Theme` from a `ThemeName` enum; all color values are compile-time constants inside the binary. |
| **Pure JSON merge** | `claude_hook::merge_stop_hook()` is a pure `Option<Value> → Value` function that injects a Z notification hook into Claude Code settings with legacy migration, deduplication, and preservation of other hooks. |

## Structure

```
src/
├── lib.rs              # Module declarations (17 pub mod)
├── domain.rs           # Core types: Project, Session, Worktree, PullRequest, Layout, Tab, Pane, ReviewStatus, etc.
├── error.rs            # ZError enum + Result<T> type alias
├── traits.rs           # Pure trait interfaces (no I/O in this crate)
├── config.rs           # KDL config parsing (3 file formats), env:VAR resolution, three-tier merging
├── action.rs           # ActionDef, ActionEnv, parse/merge/resolve, built-in actions
├── layout.rs           # Zellij KDL layout generation, prompt injection, default layout
├── theme.rs            # Theme struct, Dracula theme, Zellij KDL theme generation
├── gh.rs               # gh CLI JSON output parsing (issues, PRs, CI, reviews)
├── depcheck.rs         # DepChecker trait, semver parsing, check_deps, format_dep_error
├── activity.rs         # ActivityStore trait, SessionActivity type, session sorting by recent attach
├── notification.rs     # NotificationStore trait, session name validation, notification format
├── session_entry.rs    # Best-effort session entry effects (clear notifications + record activity)
├── claude_hook.rs      # Pure JSON merge for Claude Code .claude/settings.json Stop hook
├── log.rs              # LogEntry format/parse, Logger trait, LogLevel
├── template.rs         # Prompt template constants (issue/PR) + resolve_template
├── zellij.rs           # parse_zellij_session_info from `zellij list-sessions --json` output
└── web.rs              # Dashboard grouping helpers + Zellij web session URL builder
```

## Data & Control Flow

```
                        ┌───────────────────┐
                        │    z-core Crate    │
                        │                    │
     ┌──────────────────┼───────────────────┼──────────────────┐
     │                  │                   │                    │
     ▼                  ▼                   ▼                    ▼
┌─────────┐     ┌─────────────┐     ┌────────────┐     ┌──────────────┐
│ domain  │     │   error     │     │  traits    │     │   config     │
│ (types) │     │  (ZError)   │     │ (abstr.)   │     │ (KDL parse)  │
└────┬────┘     └─────────────┘     └──────┬─────┘     └──────┬───────┘
     │                                      │                  │
     ▼                                      ▼                  ▼
┌─────────────────────────────────────────────────────────────────────┐
│ action  ──── parse_actions_kdl → merge_actions → resolve_actions    │
│ activity ──── sort_sessions_by_recent_attach                        │
│ gh ────────── parse_gh_issues, parse_pr_view_json,                  │
│              parse_ci_status_json, parse_review_status_json          │
│ zellij ────── parse_zellij_session_info                             │
│ layout ────── generate_layout_kdl, inject_prompt_into_layout        │
│ web ───────── dashboard_groups, zellij_session_url                   │
│ template ──── resolve_template                                       │
│ theme ─────── Theme::from_name → to_zellij_kdl                      │
└─────────────────────────────────────────────────────────────────────┘
                          │
     ┌────────────────────┼────────────────────────────┐
     │                    │                            │
     ▼                    ▼                            ▼
┌──────────────┐  ┌──────────────┐  ┌─────────────────────┐
│ notification │  │ session_entry │  │   claude_hook        │
│ (store trait)│  │(effects comp.)│  │(merge_stop_hook)     │
└──────────────┘  └──────────────┘  └──────────────────────┘
                          │
                    ┌─────┴──────┐
                    ▼            ▼
            ┌──────────┐   ┌────────┐
            │ depcheck │   │  log   │
            │(checker  │   │(Logger │
            │ trait)   │   │ trait) │
            └──────────┘   └────────┘
```

### Config bootstrap flow

1. `config::parse_projects_kdl(content)` → `Vec<Project>` from `~/.config/z/projects.kdl`
2. `config::parse_global_config_kdl(content)` → `GlobalConfig` from `~/.config/z/config.kdl`
3. `config::parse_per_repo_config_kdl(content)` → `PerRepoConfig` from `<project>/.config/z.kdl`
4. `config::effective_layout(global, per_repo)` → per-repo > global > hardcoded default
5. `config::effective_issue_prompt_template(global, per_repo)` / `effective_pr_prompt_template(global, per_repo)` → per-repo > global > `DEFAULT_ISSUE_TEMPLATE` / `DEFAULT_PR_TEMPLATE`

### Action pipeline

1. KDL `action { ... }` nodes → `action::parse_actions_kdl()` → `Vec<ActionDef>`
2. `action::merge_actions(&[builtin, global_actions, per_repo_actions])` → deduped list (later overrides earlier by name, `disabled: true` removes)
3. With runtime `ActionEnv` (project, branch, PR data, CI status, review comments): `action::resolve_actions()` → `Vec<ResolvedAction>`
4. Resolve applies: context filter (Session → requires branch), condition eval (`HasPr`, `HasCiFailure`, `HasNewComments`), and `${...}` interpolation via `interpolate()`
5. `ActionPreview::from_forge_data(pr, ci_status, review)` aggregates forge data into the `ActionEnv` for project/session context

### Layout generation flow

1. `Layout` domain struct → `layout::generate_layout_kdl(layout, bin_path, theme)` → Zellij KDL string
2. Prepends `default_tab_template { tab-bar + status-bar + children }` for UI chrome
3. Appends tab definitions with pane commands and args (KDL-escaped)
4. If `session_name_env` is set, emits `env { Z_SESSION_NAME "..." }` block **after** `layout { }` block (Zellij parser rejects `env` inside `layout`)
5. Appends `keybinds` block with `Alt+k` (switcher), `Alt+l` (logs-viewer), `Alt+z` (actions) — all floating panes that close on exit
6. Appends `themes { ... }` block via `theme.to_zellij_kdl()`
7. `inject_prompt_into_layout()` appends a prompt string as an argument to the first `command="claude"` pane

### Forge data flow

1. `gh` CLI JSON → `gh::parse_gh_issues()` / `parse_gh_prs()` → `Vec<GhItem>`
2. `gh pr view --json number,state,title,url` → `parse_pr_view_json()` → `Option<PullRequest>`
3. `gh run list --json conclusion,status` → `parse_ci_status_json()` → `CiStatus`
4. `gh pr view --json reviews,latestReviews,commits` → `parse_review_status_json()` → `Option<ReviewStatus>`
5. Recursive field extraction: `collect_string_fields_into()` walks the JSON tree depth-first, collecting all `submittedAt` and `committedDate` strings
6. Review recency determined by comparing max `submittedAt` vs max `committedDate`

### Session entry flow

1. `session_entry::mark_existing_session_entered(notifications, activity, session_name)` → `SessionEntryEffects`
2. Best-effort: clears notifications + records attach timestamp; each operation is independent (failure in one doesn't abort the other)
3. `notification::validate_session_name()` rejects path-traversal characters (`/`, `\`, `..`, `.`, empty) before any file-backed adapter call

### Dependency check flow

1. `depcheck::check_deps(checker)` iterates `REQUIRED_DEPS` (`zellij >=0.44.0`, `wt >=0.34.0`, `gh >=2.0.0`)
2. Each `DepChecker::get_version_output(tool)` returns raw `--version` output (or `None` if missing)
3. `depcheck::parse_version()` extracts the first valid semver from whitespace-delimited tokens (handles `v` prefix, parenthesized versions)
4. `depcheck::format_dep_error()` produces human-readable error messages per tool

### Web dashboard flow

1. `web::dashboard_groups(sessions, projects, activity, notification_counts)` → `Vec<ProjectGroup>`
2. Filters out remote projects (those with `host` set), sessions without a matching project
3. Groups sessions by project, sorts groups alphabetically, sorts sessions by last-attach (descending)
4. `web::zellij_session_url(host, port, session)` constructs a URL with percent-encoding and IPv6 bracket wrapping

### Claude Code hook merge flow

1. `claude_hook::merge_stop_hook(existing, hook_command)` takes existing settings `Value` (or `None`)
2. Detects legacy `hooks.stop` key, migrates entries to `hooks.Stop`, wrapping plain commands in the new structure
3. Removes any existing Z hook (identified by `"z notify"` prefix) before appending the new one
4. Preserves all non-Z hooks and unrelated settings keys

## Integration Points

| Interface | Consumer(s) | Description |
|---|---|---|
| `ProjectStore` | `z-cli`, `z-tui` | Read-only CRUD for persisted projects |
| `ProjectStoreWriter` | `z-cli`, `z-tui` | Write-side project operations (add, update, remove, swap) |
| `SessionManager` | `z-cli`, `z-tui` | Zellij session lifecycle (list, create, attach, detach, kill) |
| `WorktreeManager` | `z-cli` | `wt` worktree create/list/remove |
| `ForgeClient` | `z-cli`, `z-tui` | GitHub PR, CI, review queries (backed by `gh` CLI) |
| `Notifier` | `z-cli`, `z-tui` | System notification dispatch (macOS, Telegram, TUI) |
| `SessionRefresher` | `z-tui` | Periodic background fetch of all sessions + notifications + activity |
| `DepChecker` | `z-cli` | Version probing for external tool dependencies |
| `Logger` | `z-cli`, `z-tui` | Appending structured log entries |
| `NotificationStore` | `z-cli`, `z-tui` | File-backed notification persistence per session |
| `ActivityStore` | `z-cli`, `z-tui` | File-backed last-attach timestamp tracking |
| `ConfigEnvironment` | `config` | `env:VAR` resolution strategy (injected for tests) |

### Config files consumed (parsed in this crate, read by consumers)

| File | Parser | Produces |
|---|---|---|
| `~/.config/z/projects.kdl` | `parse_projects_kdl()` | `Vec<Project>` |
| `~/.config/z/config.kdl` | `parse_global_config_kdl()` | `GlobalConfig` |
| `<project-root>/.config/z.kdl` | `parse_per_repo_config_kdl()` | `PerRepoConfig` |

### External CLI output parsed

| Source | Parser | Produces |
|---|---|---|
| `zellij list-sessions --json` | `zellij::parse_zellij_session_info()` | `Option<ZellijSessionInfo>` |
| `gh issue list --json number,title,body,url` | `gh::parse_gh_issues()` | `Vec<GhItem>` |
| `gh pr list --json ...` | `gh::parse_gh_prs()` | `Vec<GhItem>` |
| `gh pr view --json number,state,title,url` | `gh::parse_pr_view_json()` | `Option<PullRequest>` |
| `gh run list --json conclusion,status` | `gh::parse_ci_status_json()` | `CiStatus` |
| `gh pr view --json reviews,latestReviews,commits` | `gh::parse_review_status_json()` | `Option<ReviewStatus>` |
| `zellij --version` / `wt --version` / `gh --version` | `depcheck::parse_version()` | `Option<Version>` |

### Generated output consumed by Zellij

| Generator | Format | Used by |
|---|---|---|
| `layout::generate_layout_kdl()` | Zellij KDL layout string | `zellij --layout` |
| `theme.to_zellij_kdl()` | Zellij `themes { ... }` block | Appended to layout |

### Generated output consumed by Claude Code

| Generator | Format | Used by |
|---|---|---|
| `claude_hook::merge_stop_hook()` | `serde_json::Value` | Serialized to `.claude/settings.json` |

### Dependencies

| Crate | Purpose |
|---|---|
| `thiserror` | `ZError` derive macros |
| `semver` | Dependency version parsing and requirement matching in `depcheck` |
| `kdl` | KDL config document parsing for all config files and action definitions |
| `serde_json` | JSON value parsing for `gh` CLI output and Claude Code settings |
