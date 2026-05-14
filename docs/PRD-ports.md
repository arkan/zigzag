# PRD — `zigzag ports`

## Problem Statement

When working on a project, multiple processes listen on TCP ports: dev servers, databases, Docker services. There is no quick way to see which ports are in use **for the current project** without manually running `ss`, `docker ps`, and cross-referencing PIDs with working directories. This context-switching breaks flow, especially when managing multiple projects in parallel via Zellij sessions.

## Solution

A new `zigzag ports` command that lists all TCP ports in LISTEN state associated with the current project directory — covering native processes, Docker Compose services, and standalone Docker containers. The command is also exposed as a built-in action in the action menu, opening in a floating Zellij pane for quick access.

## User Stories

1. As a developer, I want to run `zigzag ports` from my project directory and see all listening ports, so that I know which services are running.
2. As a developer, I want to run `zigzag ports --project myapp` to see ports for a specific project, so that I can check without navigating to that directory.
3. As a developer, I want native processes matched by their cwd (with cmdline fallback), so that only ports relevant to my project appear.
4. As a developer, I want Docker Compose services matched via the `com.docker.compose.project.working_dir` label, so that compose ports are reliably associated.
5. As a developer, I want standalone `docker run` containers matched via their parent process cwd, so that manually launched containers also appear.
6. As a developer, I want the output grouped by source (Native / Compose / Docker), so that I can quickly identify what's what.
7. As a developer, I want to see PID, process name, port, container name, service, and image where applicable, so that I have enough context at a glance.
8. As a developer, I want a "Ports" action in the action menu, so that I can check ports without leaving my Zellij session.
9. As a developer, I want the action to open in a floating pane, so that it doesn't disrupt my layout.
10. As a developer, I want `zigzag ports` to work gracefully when Docker is not installed, so that it still shows native ports without errors.
11. As a developer, I want a clear message when no ports are found, so that I know the scan worked but nothing matched.
12. As a developer, I want inaccessible processes to be silently skipped, so that permission issues don't clutter the output.

## Implementation Decisions

### Architecture — trait in zigzag-core, impl in zigzag-cli

Following the project's strict I/O separation pattern, the port scanning logic is split:

- **zigzag-core**: defines the `PortScanner` trait, data types (`ListeningPort`, `PortSource`), and output formatting logic. No I/O.
- **zigzag-cli**: implements `SsPortScanner` which shells out to `ss -tlnp` and `docker` commands, reads `/proc` filesystem.

### Data types (zigzag-core)

- `PortSource` enum: `Native`, `Compose`, `Docker` — identifies how the port was discovered.
- `ListeningPort` struct: port number, source, PID (optional), process name (optional), container name (optional), service name (optional, Compose only), image (optional, Docker/Compose).

### Native process matching

1. Parse `ss -tlnp` output to get (port, PID, process name) tuples.
2. For each PID, read `/proc/PID/cwd` via `readlink`.
3. If cwd starts with the project path → match.
4. If not, fallback: read `/proc/PID/cmdline` and check if the project path appears in the arguments.

### Docker Compose matching

1. Run `docker ps --filter "label=com.docker.compose.project.working_dir=<project_path>" --format json`.
2. Parse JSON output for container name, service name, image, and published ports.

### Docker standalone matching

1. Run `docker ps --format json` for all running containers.
2. Exclude containers that have the Compose label (already handled above).
3. For each remaining container, get the host-side PID via `docker inspect`.
4. Walk up the process tree (`/proc/PID/status` → PPid) to find the original `docker run` process.
5. Check if any ancestor's cwd matches the project path.

### CLI interface

- `zigzag ports` — scan for current directory (`std::env::current_dir()`)
- `zigzag ports --project <name>` — resolve project path via `KdlProjectStore`

### Action menu integration

Add a built-in action in `builtin_actions()`:
- Name: "Ports"
- Type: `Run { command: "zigzag ports --project ${project_name}" }`
- Condition: `Always`
- Context: `Project`
- Pane: `Float`
- Icon: 🔌

### Output format

Grouped table printed to stdout with ANSI colors:

```
 Native processes (cwd: /home/user/myproject)
 PORT   PID    PROCESS
 3000   12345  node
 5432   6789   postgres

 Docker Compose (docker-compose.yml)
 PORT   CONTAINER         SERVICE    IMAGE
 8080   myproject-web-1   web        nginx:alpine

 Docker (standalone)
 PORT   CONTAINER         IMAGE
 9090   prometheus        prom/prometheus
```

If no ports found: "No listening ports found for this project."

### Error handling

- Docker not installed: skip Docker sections silently.
- `/proc/PID/cwd` unreadable (permissions): skip that PID.
- `ss` not available: return an error (it's expected on Linux).

## Testing Decisions

Good tests verify external behavior through the module's public interface, not implementation details. Tests should be deterministic and not depend on actual running processes or Docker state.

### Modules tested

1. **zigzag-core port types and formatting** — Unit tests for output formatting logic. Given a list of `ListeningPort` structs, verify the formatted table string is correct (grouping, column alignment, empty state message).

2. **zigzag-cli ss output parsing** — Unit tests with captured `ss -tlnp` output samples. Verify correct extraction of (port, PID, process name) tuples. Cover edge cases: IPv6 addresses, multiple processes on same port, truncated process names.

3. **zigzag-cli Docker output parsing** — Unit tests with captured `docker ps --format json` output samples. Verify correct extraction of container info, port mappings, Compose labels. Cover: multi-port containers, no ports published, Compose vs standalone distinction.

4. **zigzag-cli process matching** — Unit tests for the cwd/cmdline matching logic. Mock `/proc` reads. Verify: exact cwd match, cmdline fallback, no match, permission error handling.

5. **zigzag-core trait mock** — Integration test using a mock `PortScanner` to verify `cmd_ports` end-to-end formatting without real system calls.

### Prior art

The project already uses unit tests in zigzag-core for pure logic (action parsing, config parsing) and integration-style tests in zigzag-cli for command output. Follow the same patterns.

## Out of Scope

- **Watch/live mode** — No auto-refresh. Snapshot only for v1.
- **UDP ports** — TCP only.
- **Remote projects** — No SSH port scanning on remote hosts.
- **Port forwarding** — No automatic Zellij port-forward or tunnel creation.
- **Ports inside containers** — Only host-mapped ports are shown, not ports listening only inside the container network.
- **Process tree visualization** — No parent/child process display.

## Further Notes

- The `ss` command is part of `iproute2`, installed by default on virtually all Linux distributions.
- The `--project` flag reuses the existing `KdlProjectStore` resolution, no new config needed.
- The action uses `${project_name}` variable interpolation already supported by the action system.
- Future extensions could include: watch mode, clicking a port to open in browser (OSC 8 hyperlink), remote project support via SSH.
