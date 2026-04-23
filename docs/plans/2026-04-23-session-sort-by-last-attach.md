# Session list sorted by last attach (descending)

## Goal
Sessions in the TUI sorted so most recently attached session is at the top.

## Approach
Track last-attach timestamp per session ourselves (zellij doesn't expose it).

## Storage
- Path: `~/.config/z/session-activity.json` (colocated with `projects.kdl`)
- Format: `{ "project:branch": unix_ts_secs, ... }`
- Atomic writes (tmp + rename) to avoid corruption on concurrent writers.

## New module: `z-core/src/activity.rs`
- `activity_file_path() -> PathBuf`
- `record_attach(session_name) -> Result<()>` — upsert `now()`
- `load_activity() -> HashMap<String, u64>` — read file, empty if missing/invalid
- `remove_entry(session_name) -> Result<()>` — for kill cleanup
- `sort_sessions_by_recent_attach(&mut [Session], &HashMap<String,u64>)` — stable sort, descending by ts; missing entries → bottom (treated as 0)

## Write sites (record attach)
- `z-cli/src/main.rs::cmd_open()` — top of function, before local/remote routing. Covers: TUI opens, CLI `z open`, both attach-existing and create-new paths.
- `z-cli/src/main.rs::cmd_open_remote()` — before SSH spawn, using expected remote session name `format!("{}:{}", remote_name, sanitize_branch_name(branch))`. Covers remote attaches from this machine's TUI.

## Cleanup sites (remove entry)
- `cmd_kill` when session is killed successfully (best-effort, ignore errors).

## Sort sites
- `z-cli/src/main.rs::build_entries()` — after session load, before pushing `ProjectEntry`.
- `z-tui/src/refresh.rs::merge_refresh()` — after replacing `entry.sessions`, sort each entry's sessions. Pass `&HashMap` via `RefreshData` (refresher loads it once per tick).
- `z-tui/src/lib.rs` background refresher (~line 776) — load activity snapshot once, include in `RefreshData`.

## Tests
- `activity.rs`: roundtrip write/load, upsert preserves other keys, missing file → empty, corrupt file → empty (no panic).
- `activity.rs::sort_sessions_by_recent_attach`: mixed present/missing entries, stable for ties.
- `refresh.rs::merge_refresh`: sessions ordered by timestamp descending after merge.

## Fallback for pre-existing sessions
Missing entry = timestamp 0 = sorts to bottom. Acceptable: users will naturally surface sessions as they attach. No parsing of zellij "Created:" age needed.

## Semantics for remote
Timestamp stored on the machine running the TUI. Reflects "last time I opened this session from here", not "last time anyone attached on the remote host". Acceptable and actually desirable (per-user activity view).

## Unresolved questions
- None — design is internally consistent.
