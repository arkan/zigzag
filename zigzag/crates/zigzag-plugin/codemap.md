# zigzag/crates/zigzag-plugin/

## Responsibility

**Stub crate** reserving the `zigzag-plugin` namespace for a future WASM-based Zellij plugin runtime (planned Phase 4). Currently serves as a **workspace member and dependency carrier** — declares `zigzag-core` as a dependency and exposes `src/lib.rs` as a minimal documentation root with zero runtime logic, no public API surface, and no exported items.

Once implemented, this crate will own the **host-side plugin runtime**: loading, sandboxing, and communicating with Zellij plugins compiled to WebAssembly. At present its sole function is to anchor workspace dependency resolution and permit downstream crates to declare `zigzag-plugin` in their `Cargo.toml` without yet depending on concrete plugin infrastructure.

## Design

- **Carrier crate**: Exists to be a `[workspace]` member before any implementation exists. Allows the dependency graph to name `zigzag-plugin` without paying compile cost for a runtime that isn't built yet.
- **Stub module** (`src/lib.rs`): Two-line doc comment only. No `pub`, no `mod`, no `use`, no `fn`, no `#[cfg(test)]`, no test harness.
- **Phase-gated architecture**: Implementation is intentionally deferred. The `Cargo.toml` dependency on `zigzag-core` pre-establishes expected coupling without incurring compile weight.
- **Crate-level docs**: `src/codemap.md` documents the stub status in detail; crate root `codemap.md` mirrors that analysis at the crate boundary.

## Flow

- **No data flow**: No types, no functions, no I/O, no entry points.
- **No control flow**: No traits, no branching, no event handling, no plugin lifecycle.

## Integration

- **Workspace coupling**: Declares `zigzag-core.workspace = true`, anchoring it within the monorepo's dependency graph. Any future plugin runtime will consume `zigzag-core`'s abstractions (pane model, layout, event types).
- **Crate graph position**: Dependent of `zigzag-core`; likely future consumer of `zigzag-ipc` and `zigzag-server`. Downstream consumers may include integration test harnesses or the CLI binary.
- **No FFI / linkage**: No `#[wasm_bindgen]`, no `extern`, no ABI boundary. All integration is future-planned and will involve WASM host APIs when Phase 4 begins.
