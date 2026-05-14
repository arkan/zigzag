# z/crates/z-web/src/

## Responsibility

Provide an HTTP/WebSocket server interface for the z platform, exposing core
capabilities (activity queries, layout management, Claude hook endpoints,
session orchestration) over the network. Planned as phase 5 of the z roadmap,
this crate bridges the headless CLI and TUI frontends with remote clients
(browser-based dashboards, IDE extensions, CI/CD webhooks) by hosting an
axum-based HTTP server and embedding the ratatui terminal renderer as a WASM
target for in-browser terminal emulation.

Currently a **stub placeholder** — no modules, types, or runtime logic are
implemented. The crate exists to reserve the name, establish the dependency
edge to `z-core`, and document the intended architecture before active
development begins.

## Design

- **Stub-first incremental delivery**: The crate is introduced as an empty
  library with a single dependency on `z-core`, serving as a compile-time
  scaffold. Implementation is deferred to a later phase to avoid premature
  commitment to web-framework and rendering choices.
- **Axum as HTTP framework** (declared intent): The comment in `lib.rs`
  specifies axum as the server foundation. Axum's tower-based middleware
  stack, extraction-based handlers, and typed state pattern align with the
  existing `z-core` error and trait idioms, enabling composable route
  modules.
- **ratatui → WASM rendering** (declared intent): The TUI renderer (ratatui)
  is planned for cross-compilation to WASM, allowing the same terminal layout
  and widget code to render inside a browser-based terminal emulator (e.g.,
  xterm.js). This avoids a separate web UI framework — the TUI _is_ the UI,
  streamed over WebSocket.
- **Single-crate boundary**: All web routing, middleware, WASM shims, and
  session glue live in this crate. No sub-modules currently exist; future
  module decomposition is expected (e.g., `routes/`, `wasm/`, `state/`,
  `ws/`).

## Flow

*Not applicable — crate is a stub.* The intended control flow is:

1. **Startup**: `z-web` is launched (either standalone or embedded via
   `z-cli`), initializing axum `Router` with shared `z-core` state (config,
   activity store, layout engine).
2. **Inbound HTTP/WS**: External clients connect via REST endpoints or
   WebSocket upgrade. Axum extractors deserialize requests; handlers
   delegate to `z-core` services for domain logic.
3. **WASM TUI stream**: A WebSocket route serves a virtual terminal buffer
   rendered by ratatui compiled to WASM. The browser client renders each
   frame into an xterm.js instance, providing full keyboard/mouse input
   capture forwarded back over the WebSocket.
4. **Response**: REST endpoints return JSON (via `serde`); WS frames carry
   binary/JSON terminal events.

## Integration

- **z-core** (direct dependency): Consumes config, activity, layout, error,
  and session types. Every handler and WS message translates between HTTP
  protocol and `z-core` domain models. The `error` module's `thiserror`-
  derived types map to HTTP status codes via axum's `IntoResponse`.
- **z-cli** (planned consumer): The CLI binary may embed or spawn the web
  server as a subcommand (`z web serve`), passing the shared runtime handle.
- **External clients**: REST API consumers (curl, HTTP clients) and browser
  WASM clients connect via TCP. No authentication or middleware layer is
  specified yet; these will be added when the crate is implemented.
- **Workspace position**: Eighth crate (last in member list), depends only on
  `z-core`. It is a terminal/leaf node in the dependency graph — no other
  crate depends on `z-web`.

## Current State

```
Module      Lines  Types  Functions  Status
lib.rs        2     0        0       Stub (planning placeholder)
```

The crate compiles as an empty library. No tests, examples, or benchmarks
exist. All content above describing intended behaviour is forward-looking
and subject to change during implementation.
