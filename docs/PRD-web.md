# Z — Web Dashboard PRD (`zigzag web`)

See also: [Main PRD](./PRD.md) | [Specs](./SPECS.md) | [Ports PRD](./PRD-ports.md) | [Action Menu PRD](./PRD-action-menu.md)

---

## Problem Statement

Developers using `zigzag` interact with Zellij sessions exclusively through a local terminal — either via the `zigzag` TUI or by attaching directly to Zellij. When away from the primary workstation (on an iPad, phone, laptop on another network, tablet during a meeting), there is no way to peek at or resume an active session. The existing remote-attach path (SSH + Zellij HTTPS attach) still requires a terminal emulator and SSH configuration on the consuming device, which does not exist on mobile or sandboxed environments.

Zellij 0.44 ships a native `zellij web` daemon that serves terminal sessions over HTTP + WebSocket using `xterm.js`, with token-based authentication. It is unused by `zigzag` today. Exposing this surface, cleanly integrated with `zigzag`'s project/session model, would let developers interact with any `zigzag`-managed session from any device with a modern browser, without reimplementing any terminal-streaming logic.

The problem is therefore not "how to render a terminal in a browser" — Zellij solves that — but "how to make this accessible, discoverable, and scoped to the projects `zigzag` already knows about."

## Solution

A new `zigzag web` command family that orchestrates a browser-accessible dashboard of `zigzag`-managed sessions. On `zigzag web open`, `zigzag` starts the `zellij web` daemon on demand (if not already running), starts a minimal HTTP server on a configurable port, and opens the user's default browser on the dashboard.

The dashboard itself is a single server-rendered HTML page listing the current `zigzag`-managed sessions (same filtering as the TUI), grouped by project. Each session row links to the corresponding native `zellij web` URL in a new browser tab. No terminal rendering, no WebSocket handling, and no iframe wrapping happens in `zigzag`'s server — all terminal interaction is delegated to Zellij's native web UI.

A single long-lived token, created and stored by `zigzag` under `~/.local/state/zigzag/web-token`, is used to authenticate the browser with `zellij web`. On first launch, `zigzag` opens the Zellij web URL with the token in a URL fragment (`#token=...`) so the browser's local storage picks it up silently. Subsequent launches open directly on the Z dashboard.

The feature is bound to the loopback interface by default, with an explicit opt-in for LAN exposure. Remote projects (those with a `host` defined in `projects.kdl`) are hidden in v1; support will land alongside the remote phase.

## User Stories

### Launching and Stopping

1. As a developer, I want to run `zigzag web open` from any terminal to start the web dashboard, so that I do not have to set up a systemd service or remember which daemon processes need to run.
2. As a developer, I want `zigzag web open` to be idempotent — running it when the daemon is already up just reopens the browser — so that I never have to think about state.
3. As a developer, I want `zigzag web stop` to cleanly shut down the dashboard and the Zellij web daemon together, so that I can disable the feature when I do not need it.
4. As a developer, I want `zigzag web status` to tell me whether both daemons are running and on which ports, so that I can diagnose issues quickly.
5. As a developer, I want exit codes from `zigzag web status` to reflect partial states (0 up, 1 down, 2 partial), so that I can script checks in hooks or automation.

### Dashboard Content

6. As a developer, I want the dashboard to list only the sessions that `zigzag` manages (matching the `{project}:{branch}` pattern and a project in my `projects.kdl`), so that the web view is consistent with the TUI.
7. As a developer, I want sessions grouped by project, so that related work is visually clustered.
8. As a developer, I want sessions sorted by last-attach timestamp (most recent first) within each project group, so that the session I am actively working on is always near the top.
9. As a developer, I want to see the session name, branch, uptime, and tab/pane counts per row, so that I have enough context to pick the right one.
10. As a developer, I want a notification badge next to sessions with pending `zigzag notify` entries, so that I can spot alerts from background autopilot runs at a glance.
11. As a developer, I want clicking a session to open its terminal in a new browser tab, so that the dashboard remains available for navigation while I work.
12. As a developer, I want an empty-state message with a hint (e.g. `zigzag open <project>`) when no sessions exist, so that the dashboard is not a blank wall.

### Interaction and Refresh

13. As a developer, I want a manual refresh button in the dashboard header, so that I decide when to fetch a fresh list rather than the page polling on its own.
14. As a developer, I want no automatic polling in v1, so that the page preserves scroll and selection state between my glances at it.

### Configuration

15. As a developer, I want the dashboard port to default to 8083 and the Zellij web port to default to 8082, so that the feature works with zero config out of the box.
16. As a developer, I want to override both ports in `~/.config/zigzag/config.kdl` under a `web { }` block, so that I can resolve conflicts with other local services.
17. As a developer, I want the bind address to default to `127.0.0.1` (loopback), so that no one on my LAN can reach the dashboard until I explicitly opt in.
18. As a developer, I want to set `web { host "0.0.0.0" }` to expose the dashboard on the LAN, so that I can access it from my iPad or phone on the same network.
19. As a developer, I want a clear error message when a port is already in use (with the config knob to fix it), so that I am not left guessing.

### Authentication

20. As a developer, I want `zigzag` to manage a single long-lived token named `zigzag-web`, so that I can bookmark the dashboard URL and have it keep working across reboots.
21. As a developer, I want `zigzag web rotate` to revoke the current token and generate a new one, so that I can respond to a suspected leak without rebuilding anything.
22. As a developer, I want the token stored on disk with mode `0600` under `~/.local/state/zigzag/`, so that other users on the system cannot read it.
23. As a developer, I want my browser to authenticate automatically on first launch via a URL fragment (`#token=...`), so that I do not have to paste a token into a login form.
24. As a developer, I want a fallback where `zigzag` prints the token and opens the Zellij login page if fragment auto-login is not supported, so that the feature works even if Zellij's behavior changes.
25. As a developer, I do NOT want the token transported in URL query strings, so that it does not leak into server logs or `Referer` headers.

### Discoverability

26. As a developer, I want `Alt+W` inside any Zellij session to open the dashboard, so that I can reach the web UI from any pane I happen to be in.
27. As a developer, I want `w` in the TUI to open the dashboard, so that the keybinding is consistent with the existing single-letter actions.
28. As a developer, I want an "Open web dashboard" entry in the action menu, so that I can trigger it by name if I forget the shortcut.
29. As a developer, I want the TUI footer to advertise the `[w]eb` key, so that the shortcut is self-documenting.

### Non-Goals (For User-Facing Clarity)

30. As a developer, I do NOT want `zigzag` to reimplement a terminal emulator in my browser; Zellij already does this and I trust it.
31. As a developer, I do NOT want to create, delete, or close sessions from the web dashboard in v1; the TUI remains the CRUD surface.
32. As a developer, I do NOT want remote projects (those with `host` in `projects.kdl`) shown in v1, because the remote phase has not landed yet and showing them would be misleading.
33. As a developer, I do NOT want PR, CI, or git ahead/behind status on the dashboard in v1, because it introduces network calls to `gh` that can slow or break the page on poor connections.

## Implementation Decisions

### Architecture — traits in zigzag-core, HTTP server in zigzag-web

Following the project's strict I/O separation (cf. SPECS §3.2):

- **zigzag-core**: defines pure logic for filtering sessions for the web view (Zigzag-managed + non-remote) and building Zellij web URLs given a host, port, and session name. Adds no new I/O trait unless needed; reuses `SessionManager`, `ProjectStore`, `Notifier` already defined there.
- **zigzag-web**: the new HTTP server crate. Depends on `axum`, `tokio`, `maud` (HTML templating), `open` (to launch the browser), and `zigzag-core`. Implements the handlers that wire the `zigzag-core` traits to HTTP responses.
- **zigzag-cli**: adds the `web` subcommand tree (`open`, `stop`, `status`, `rotate`). Orchestrates the `zellij web` daemon lifecycle via `zellij web --daemonize / --status / --stop / --create-token / --revoke-token`. Launches the `zigzag-web` HTTP server as a background process or child task.

No logic related to HTTP, templating, or the filesystem representation of the token leaks into `zigzag-core`.

### Delegation Model

`zigzag-web` serves only a dashboard index page and optionally a tiny JSON endpoint for future use. The terminal itself is never streamed through `zigzag-web`. Each session row in the HTML is an `<a target="_blank">` pointing to the native `zellij web` URL. This preserves all Zellij-native UX (resize, multiplayer cursors, shortcuts) and eliminates any X-Frame-Options / CSP coupling.

### Daemon Lifecycle

- `zigzag web open` runs a start-on-demand sequence: check `zellij web --status`, start it with `--daemonize` if absent; check the `zigzag-web` server, start it if absent; ensure the `zigzag-web` token exists, create it if absent; open the browser.
- Status discovery relies on `zellij web --status` plus a PID file written by `zigzag-web` at `~/.local/state/zigzag/web.pid`.
- `zigzag web stop` stops both daemons in reverse order.
- `zigzag web open` is idempotent: subsequent calls reopen the browser without relaunching daemons.

### First-Run Authentication Flow

1. If `~/.local/state/zigzag/web-authenticated-at` does not exist: open the browser on `http(s)://<host>:<zellij-port>/#token=<token>` so Zellij's web UI captures the token into the browser's local storage. Write the timestamp file.
2. If it exists: open the browser on the `zigzag-web` dashboard directly.
3. A spike must verify that Zellij supports fragment-based auto-login. If not, fall back to printing the token in the terminal and opening the Zellij login page for manual paste. The rest of the flow is unchanged.

### Token Management

- Single named token, `zigzag-web`, created via `zellij web --create-token --token-name zigzag-web`.
- Stored at `~/.local/state/zigzag/web-token` with mode `0600`. This is the source of truth for the token value; Zellij holds it server-side for validation but never exposes it back.
- `zigzag web rotate` calls `--revoke-token zigzag-web` then `--create-token --token-name zigzag-web`, and rewrites the state file. Existing browser sessions are invalidated and must reauthenticate.
- Tokens are passed to browsers only via URL fragments, never query strings. The fragment is consumed by Zellij's web JS and persisted in `localStorage`; the server never sees it.

### CLI Surface

| Command | Behavior |
|---|---|
| `zigzag web open` | Start daemons if needed, ensure token, open browser on dashboard. |
| `zigzag web stop` | Stop `zigzag-web` server, then `zellij web` daemon. |
| `zigzag web status` | Print daemon and server state, ports, URLs, token name and creation date. Exit 0 (all up), 1 (all down), 2 (partial). |
| `zigzag web rotate` | Revoke and recreate the `zigzag-web` token; invalidate existing browser sessions. |
| `zigzag web` (no sub) | Print help. Does NOT default to `open`, to keep the `zigzag <verb>` pattern consistent. |

### Configuration

Added to `~/.config/zigzag/config.kdl`. No per-repo override (the dashboard is machine-global, not project-scoped).

```kdl
web {
    port 8083               // Z dashboard port
    zellij-port 8082        // zellij web daemon port
    host "127.0.0.1"        // bind address; set "0.0.0.0" to expose on LAN
}
```

All fields optional with the defaults above.

### Dashboard Page

Single route `GET /`. Server-renders the full HTML page with:

- A header showing the machine hostname, the Z dashboard URL (for copy/paste to another device), and a `[↻ Refresh]` button (a plain `<a href="/">`).
- Sessions grouped by project heading, sorted by last-attach descending within each group.
- Per row: session name, branch, notification count badge, session uptime, tab/pane count, a right-aligned link to the Zellij session URL.
- Empty state when no sessions exist: a message and a hint (`zigzag open <project>` with the list of known projects).

All content is derived from local state: `SessionManager::list_sessions()`, the activity log (`zigzag-core/src/activity.rs`), and the notification files under `~/.local/state/zigzag/notifications/`. No calls to `gh`, git, or any forge API.

### Styling

Inline CSS in the maud template, under 2 KB. No CSS framework, no external fonts. Dark theme by default matching the Dracula palette used elsewhere in `zigzag`. Respects `prefers-color-scheme`.

### Action Menu Integration

A new built-in action with:

- Name: `Open web dashboard`
- Icon: 🌐
- Kind: `Run` with command `zigzag web open`
- Condition: `Always`
- Context: `Global` (not per-project — the dashboard is transverse)
- Pane: `Tab`

### TUI Keybinding Integration

- `w` in the TUI (outside search mode) runs `zigzag web open` as a fire-and-forget command. The TUI remains on screen.
- `Alt+W` is injected into Zellij sessions alongside the existing `Alt+Z / Alt+K / Alt+L` triad, bound to `Run { command: ["zigzag", "web", "open"] }` via the session keybinding module.
- The TUI footer gains a `[w]eb` hint next to the existing entries.

### Logging

`zigzag-web` writes structured logs to `~/.local/state/zigzag/web.log` with rotation (size-based, keep last 5 files). These logs are surfaced by `zigzag logs` under the `web` tag.

### Port Conflict Handling

If either port is bound at startup, fail fast with a message that names the port and the config knob to change it. No silent port-hopping fallback.

### Hidden-for-Now Remote Projects

The session-filtering helper in `zigzag-core` excludes any session whose project has a non-empty `host` field. When the remote phase lands, this filter will be extended to emit external links pointing at the remote machine's own `zellij web`.

## Testing Decisions

Good tests verify external behavior through the module's public interface, not implementation details. Tests must be deterministic and must not spawn real `zellij` or HTTP servers.

### Prior Art

The project already uses unit tests in `zigzag-core` for pure logic (action parsing, config parsing, project-session matching) and integration-style tests in `zigzag-cli` for command output. The same patterns extend to `zigzag-web`.

### Modules Tested

1. **zigzag-core — session filtering for web**. Unit tests for the pure helper that takes `(sessions, projects)` and returns the list suitable for the dashboard (Zigzag-managed, non-remote, sorted by last-attach). Cover: all-local, mix of local and remote, sessions without a matching project, empty inputs.
2. **zigzag-core — Zellij URL building**. Unit tests for the helper that constructs the per-session Zellij web URL. Cover: plain names, names with `/` and `:`, names with non-ASCII, IPv6 hosts, default vs custom ports.
3. **zigzag-web — template rendering**. Unit tests that render the index page from a fixture list of sessions and assert on targeted substrings: project name, branch, notification badge, link format. No pixel-level HTML snapshots.
4. **zigzag-web — empty state rendering**. Unit test for the zero-sessions case asserting the hint message and the list of known projects are present.
5. **zigzag-web — HTTP smoke tests**. Using `axum::Router` + `tower::ServiceExt::oneshot`, build the app with a mock `SessionManager` / `ProjectStore`, issue `GET /`, assert status 200, content-type `text/html`, and that the body contains expected session names and Zellij link URLs.
6. **zigzag-cli — web subcommand dispatch**. Unit tests for `zigzag web status` with mocked daemon-state reporters returning all-up, all-down, and partial states; assert stdout format and exit code.
7. **zigzag-cli — token rotation orchestration**. Unit tests for the `rotate` flow using a mock `zellij web` invoker; assert `--revoke-token` then `--create-token` are called in order and the state file is rewritten.

### Not Tested Automatically

- First-run fragment-based auto-login. Behavior depends on Zellij's web JS, which is not under `zigzag`'s control. Covered by a manual checklist in the PRD and a spike (see Further Notes).
- Opening the default browser. The `open` crate is responsible; mocked out with a trait boundary.
- End-to-end with a real `zellij web` daemon in CI. Excluded to keep CI deterministic and fast. A `make smoke-web` target documented for local manual verification is acceptable.

## Out of Scope

- **Terminal streaming inside `zigzag-web`**. Delegated to `zellij web`. Not considered.
- **Iframe embedding of Zellij sessions**. Terminal views always open in a new tab, to sidestep X-Frame-Options / CSP coupling.
- **CRUD on sessions from the web**. No open, close, delete, or prune buttons in v1. The TUI remains authoritative.
- **PR, CI, or git ahead/behind status in the list**. Requires forge calls; out of scope to keep the dashboard fast and offline-friendly.
- **Auto-refresh / live updates**. No polling, no WebSocket push. Manual refresh only in v1; htmx-driven polling is the documented upgrade path.
- **Remote projects**. Hidden in v1. Revisits when the remote phase lands.
- **ratatui-WASM embedding of the full TUI**. That is phase 5 of the main specs; out of scope here.
- **Action menu, notifications, autopilot UI on the web**. TUI-only in v1.
- **Multi-user auth beyond the single `zigzag-web` token**. Zellij's read-only tokens are not surfaced by `zigzag` in v1.
- **TLS termination in `zigzag-web`**. If the dashboard is exposed beyond loopback, the user is expected to front it with a tunnel (Tailscale, wireguard, Cloudflare Tunnel) rather than have `zigzag` handle certificates.
- **Per-repo `web { }` overrides**. The dashboard is machine-global; no repo config for it.

## Further Notes

### Spike Items (Resolve Before Implementation)

These require empirical verification against Zellij 0.44.x, not design decisions:

1. **Does `zellij web` support `#token=<value>` URL fragments for silent auto-login?** If yes: the first-run flow works as designed. If no: fall back to printing the token and opening the login page for manual paste.
2. **What is the exact URL shape for deep-linking a specific session in `zellij web`?** Candidates include `/?session=NAME`, `/sessions/NAME`, or `/#/session/NAME`. Needed to generate correct `<a>` targets in the dashboard.
3. **What `X-Frame-Options` / `Content-Security-Policy` headers does `zellij web` emit?** Even though v1 uses `target="_blank"` and not iframes, documenting the headers keeps a future "Z-chromed iframe" option on the table.
4. **Stability of `zellij list-sessions` output while the web daemon is running.** Verify that session lifecycle events seen through the web daemon do not cause inconsistent entries in the list used to build the dashboard.

### Relationship to Existing Phasing

This PRD implements a subset of what SPECS §11 called "phase 5 — Web UI (ratatui WASM + xterm.js, Leptos fallback + axum)". The delta: ratatui-WASM and a full web TUI port are deferred. Only the session-index dashboard and the delegation to `zellij web` ship now. When phase 5 is reactivated, `zigzag-web` already exists as the host crate.

### Dependencies Introduced

- `axum` — HTTP routing and handlers.
- `tokio` — async runtime (already transitively present).
- `maud` — compile-time typed HTML templating.
- `open` — cross-platform browser launcher.

No JavaScript build pipeline. No bundler. No `node_modules`. Everything remains `cargo build`.

### Security Posture

- Default bind `127.0.0.1` prevents LAN exposure until opted in.
- Token stored with mode `0600`.
- Token transported only via URL fragment (client-side), never query string.
- `zigzag web rotate` as the lever for leak response.
- Out-of-scope — and explicitly so — any network-facing TLS, OAuth, or per-user access control. Users needing remote access are expected to use a private tunnel.

### Future Extensions

- **htmx-driven live refresh**. One endpoint `/partials/sessions` returning a list fragment, polled every 3s by a `hx-trigger="every 3s"` attribute. Zero architectural change, drop-in addition.
- **Session CRUD from the web**. Wire the TUI's create/delete flows to POST endpoints. Requires revisiting CSRF and the single-token auth model.
- **Remote project linking**. Once SSH + remote Zellij web is stable, the dashboard can emit external links per-project pointing at the remote daemon's URL.
- **ratatui-WASM TUI**. Replace the static HTML dashboard with a compiled Rust/WASM binary that reuses the same `zigzag-core` traits as the native TUI.
- **OSC 8 hyperlink output for `zigzag web status`**. A `zigzag web status --url` flag or automatic hyperlinking of the dashboard URL when printed to a supporting terminal.
