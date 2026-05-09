# OpenCode notification integration

## Goal

Notify the correct `z` Session when OpenCode finishes work or asks for user intervention.

## Plan

- [x] Add stable Session env resolution for `z notify`: explicit arg → `Z_SESSION_NAME` → `ZELLIJ_SESSION_NAME`.
- [x] Emit `Z_SESSION_NAME` into generated Zellij layouts via a layout-level `env` block.
- [x] Update Claude Stop hook injection to use `Z_SESSION_NAME` while preserving Zellij fallback.
- [x] Add project-local OpenCode plugin at `.opencode/plugins/z-notify.js` for `session.idle`, `permission.asked`, `session.error`.
- [x] Add/update focused tests for notify resolution, layout env output, hook command.
- [x] Verify with `cargo test --manifest-path z/Cargo.toml --all`.

## Review

- `z notify "message"` now resolves via `Z_SESSION_NAME` before `ZELLIJ_SESSION_NAME`.
- Generated Zellij layouts set `Z_SESSION_NAME` for all panes in the session.
- OpenCode project plugin notifies on finish, permission request, and error; supports both `session.idle` and `session.status` idle payloads, and both event/direct permission hook shapes.
- Verification: `node --check .opencode/plugins/z-notify.js`, `cargo fmt --all`, and `cargo test --all` passed.

## Unresolved questions

- None. Use the repo-local OpenCode plugin path documented by OpenCode: `.opencode/plugins/`.
