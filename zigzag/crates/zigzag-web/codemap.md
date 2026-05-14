# z/crates/z-web/

## Responsibility

Provide an HTTP/WebSocket server interface for the z platform, exposing core
capabilities (activity queries, layout management, Claude hook endpoints,
session orchestration) over the network. Planned as phase 5 of the z roadmap,
this crate bridges the headless CLI and TUI frontends with remote clients
(browser-based dashboards, IDE extensions, CI/CD webhooks).

**Current status**: Stub placeholder. No modules, types, or runtime logic are
implemented. The crate exists to reserve the name, establish the dependency
edge to `z-core`, and document the intended architecture.

## Design Patterns

- **Axum-based HTTP server** (declared intent): Axum's tower-based middleware
  stack, extraction-based handlers, and typed state pattern align with the
  existing `z-core` error and trait idioms, enabling composable route modules.
- **WASM TUI rendering** (declared intent): The ratatui terminal renderer is
  planned for cross-compilation to WASM, allowing the same terminal layout
  and widget code to render inside a browser-based terminal emulator
  (e.g., xterm.js), streamed over WebSocket. The TUI _is_ the UI — no
  separate web framework.
- **Single-crate boundary**: All web routing, middleware, WASM shims, and
  session glue live in this crate. No sub-modules currently exist; future
  decomposition is expected (e.g., `routes/`, `wasm/`, `state/`, `ws/`).
- **Stub-first incremental delivery**: Introduced as an empty library with a
  single dependency on `z-core`, serving as a compile-time scaffold.
  Implementation is deferred to avoid premature commitment to web-framework
  and rendering choices.
- **Leaf-node dependency**: Eighth crate in the workspace member list, depends
  only on `z-core`. No other crate depends on `z-web` — it is a terminal
  node in the dependency graph.

## Data & Control Flow

*Not yet implemented — the following is the intended architecture:*

1. **Startup**: `z-web` is launched (standalone or embedded via `z-cli`),
   initializing an axum `Router` with shared `z-core` state (config, activity
   store, layout engine).
2. **Inbound HTTP/WS**: External clients connect via REST endpoints or
   WebSocket upgrade. Axum extractors deserialize requests; handlers
   delegate to `z-core` services for domain logic.
3. **WASM TUI stream**: A WebSocket route serves a virtual terminal buffer
   rendered by ratatui compiled to WASM. The browser client renders each
   frame into an xterm.js instance, with keyboard/mouse input captured and
   forwarded back over the WebSocket.
4. **Response**: REST endpoints return JSON (via `serde`); WS frames carry
   binary/JSON terminal events.

## Integration Points

- **z-core** (only dependency): Consumes config, activity, layout, error,
  and session types. Every handler and WS message translates between HTTP
  protocol and `z-core` domain models. The `error` module's `thiserror`-
  derived types map to HTTP status codes via axum's `IntoResponse`.
- **z-cli** (planned consumer): The CLI binary may embed or spawn the web
  server as a subcommand (`z web serve`), passing the shared runtime handle.
- **External clients**: REST API consumers (curl, HTTP clients) and browser
  WASM clients connect via TCP. No authentication or middleware is specified
  yet — these will be added when the crate is implemented.
- **Workspace position**: Terminal/leaf node — depends only on `z-core` and
  is not depended upon by any other crate.
