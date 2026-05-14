# zigzag/crates/zigzag-plugin/src/

## Responsibility

**Stub crate** for a future WASM-based Zellij plugin (planned for Phase 4). Currently serves as a **placeholder and dependency carrier** — it declares `zigzag-core` as a dependency and exposes `lib.rs` as a documentation root, but contains zero runtime logic, no public API surface, and no exported items.

Once implemented, this crate will own the **host-side plugin runtime**: loading, sandboxing, and communicating with Zellij plugins compiled to WebAssembly. At present its sole function is to reserve the namespace and anchor workspace dependency resolution.

## Design Patterns

- **Carrier crate**: Exists to be a `[workspace]` member and dependency target before any implementation exists. Permits downstream crates to declare `zigzag-plugin` as a dependency without yet depending on concrete plugin infrastructure.
- **Stub module** (`lib.rs`): Minimal — a single doc comment. No `pub`, no `mod`, no `use`, no `fn`. No `#[cfg(test)]` or test harness.
- **Phase-gated architecture**: Intentionally empty; implementation is deferred to a later phase. The crate's `Cargo.toml` dependency on `zigzag-core` pre-establishes the expected coupling without incurring compile weight.

## Data & Control Flow

- **No data flow**: No types, no functions, no I/O.
- **No control flow**: No entry points, no traits, no branching.

## Integration Points

- **Workspace coupling**: Declares `zigzag-core.workspace = true`, anchoring it within the monorepo's dependency graph. Any future plugin runtime will consume `zigzag-core`'s abstractions (pane model, layout, event types).
- **Crate graph position**: Dependent of `zigzag-core`; likely consumer of `zigzag-ipc` and `zigzag-server` when implemented. Downstream consumers may include an integration test harness or CLI binary.
- **No FFI / linkage**: No `#[wasm_bindgen]`, no `extern`, no ABI boundary. All integration is future-planned.
