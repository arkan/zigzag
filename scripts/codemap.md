# scripts/

Build/devops scripts for the `zigzag` project — Docker sandbox setup, runtime patching, and auth orchestration.

## Responsibility

Provide the shell-level substrate for the sandcastle-based agent sandboxing system:
1. **`patch-sandcastle.sh`** — Post-install patching of the `@ai-hero/sandcastle` npm package to fix macOS Docker Desktop compatibility (VirtioFS bind-mount `chown` errors) and inject a persistent auth volume into every sandbox container.
2. **`sandcastle-auth.sh`** — Idempotent Docker-based authentication init: creates the shared auth volume, checks if Claude CLI credentials already exist, and runs `claude setup-token` interactively when needed.

Both scripts are glue between the Node.js sandcastle package and the local Docker environment — they are *not* part of the Rust application runtime.

## Design

- **Patch-via-sed+Python (not config or forks):** Rather than forking `@ai-hero/sandcastle`, the project mutates dist files with targeted string replacement. This keeps the dependency pinned but makes the patches fragile across version bumps (`sed` + Python `str.replace` on minified-esque JS).
- **Docker volume for auth persistence:** Credentials live in a named Docker volume (`sandcastle-claude-auth`) rather than bind-mounting host `~/.claude`, decoupling auth state from the host filesystem and making it portable across machines.
- **Idempotent auth check:** `sandcastle-auth.sh` probes `claude auth status` inside the container and short-circuits if already authenticated — avoids unnecessary interactive prompts.
- **`postinstall` lifecycle hook:** Patching happens automatically on `npm install` / `npm ci`, ensuring sandcastle is always patched in local dev environments.

## Flow

### patch-sandcastle.sh

```
npm install (postinstall hook)
  └─ scripts/patch-sandcastle.sh
       ├─ Patch 1: chownInContainer() in DockerLifecycle.js
       │     Replace chown exec call with --silent flag + Effect.catchAll(() => void)
       │     so that VirtioFS chown failures on macOS are silently ignored
       ├─ Patch 2: SandboxFactory.js
       │     Spread sandcastle-claude-auth volume into volumeMounts array
       └─ Patch 3: createSandbox.js
             Same volume injection as Patch 2 (second code path)
```

### sandcastle-auth.sh

```
Terminal (manual invocation)
  └─ scripts/sandcastle-auth.sh
       ├─ docker volume inspect → create if missing
       ├─ docker run --rm (ephemeral) to check `claude auth status`
       ├─ If authenticated → exit 0
       └─ If not → docker run --rm -it (interactive) → `claude setup-token`
```

## Integration

| Script | Trigger | Depends On | Affects |
|---|---|---|---|
| `patch-sandcastle.sh` | `postinstall` in `package.json` | `@ai-hero/sandcastle` npm package installed in `node_modules/` | `node_modules/@ai-hero/sandcastle/dist/DockerLifecycle.js`, `SandboxFactory.js`, `createSandbox.js` |
| `sandcastle-auth.sh` | Manual (developer runs it) | Docker daemon, `sandcastle:z` image (built from `.sandcastle/Dockerfile`), named volume `sandcastle-claude-auth` | Docker named volume `sandcastle-claude-auth` (created/read) |

- **Rust workspace:** Not integrated — these scripts are Node.js/Docker infrastructure only.
- **`.sandcastle/` directory:** The patched sandcastle runtime and the Docker image used by `sandcastle-auth.sh` are defined in `.sandcastle/Dockerfile` and `.sandcastle/main.ts`.
- **Environment:** Both scripts assume a Docker Desktop setup; `patch-sandcastle.sh` specifically targets macOS VirtioFS limitations. Not intended for CI or production use.
