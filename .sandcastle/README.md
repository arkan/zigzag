# Sandcastle — AI Agent Orchestration

[Sandcastle](https://github.com/mattpocock/sandcastle) runs isolated Claude Code agents in Docker containers to work on GitHub issues in parallel.

## How it works

`main.ts` runs a 3-phase loop (up to 10 iterations):

1. **Plan** (Opus) — reads open GitHub issues labeled `Sandcastle`, builds a dependency graph, selects unblocked issues
2. **Execute + Review** (Sonnet + Opus) — implements each issue in parallel (TDD), then reviews the code
3. **Merge** (Sonnet) — merges all completed branches into `main`, resolves conflicts

Each agent runs in its own Docker container with an isolated git worktree.

## Prerequisites

- Docker Desktop
- Node.js 18+
- A GitHub token with repo access

## Setup

### 1. Install dependencies

```bash
npm install
```

The `postinstall` script automatically patches Sandcastle for macOS Docker Desktop compatibility (VirtioFS chown fix + persistent auth volume).

### 2. Authenticate Claude Code

Sandcastle agents need to authenticate with Anthropic. Two options:

#### Option A: Claude Max/Pro subscription (interactive login)

```bash
make login
```

This opens a shell inside the Docker container. Run `claude`, follow the login URL, authenticate in your browser, then `exit`. Credentials are stored in a persistent Docker volume (`sandcastle-claude-auth`) and reused across all future runs.

#### Option B: API key

Add your Anthropic API key to `.sandcastle/.env`:

```
ANTHROPIC_API_KEY=sk-ant-...
GH_TOKEN=ghp_...
```

### 3. Configure GitHub

Add your GitHub token to `.sandcastle/.env`:

```
GH_TOKEN=ghp_...
```

## Usage

```bash
make sandcastle    # build Docker image + run
make login         # authenticate Claude Code in container
make auth-status   # check authentication status
make docker        # build Docker image only
```

## Files

| File | Purpose |
|------|---------|
| `main.ts` | Orchestration script (plan → execute → review → merge) |
| `plan-prompt.md` | Planner agent prompt — analyzes issues, picks parallelizable work |
| `implement-prompt.md` | Implementer agent prompt — TDD workflow per issue |
| `review-prompt.md` | Reviewer agent prompt — edge cases, code quality |
| `merge-prompt.md` | Merger agent prompt — merges branches, resolves conflicts |
| `Dockerfile` | Container image with Node, Git, gh CLI, Claude Code |
| `.env` | Credentials (gitignored) |

## macOS Notes

The `postinstall` script (`scripts/patch-sandcastle.sh`) applies two patches:

1. **chown fix** — Docker Desktop's VirtioFS doesn't allow `chown` on bind-mounted files. The patch makes chown errors non-fatal.
2. **Auth volume mount** — Injects the `sandcastle-claude-auth` Docker volume into every container so agents share the same credentials.

These patches are reapplied automatically on every `npm install`.
