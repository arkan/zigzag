---
name: release
description: Bump version, tag, and push to trigger a GitHub Release with cross-platform binaries.
---

# Release

Create a new GitHub Release by bumping the workspace version, committing, tagging, and pushing. The push triggers the `release.yml` workflow which builds cross-platform binaries and publishes the release.

## Arguments

One required argument: `patch`, `minor`, or `major`.

## Workflow

### 1. Pre-flight checks

Abort with a clear error message if any check fails.

```bash
# Must be on main
[ "$(git branch --show-current)" = "main" ] || { echo "ERROR: not on main branch"; exit 1; }

# Working tree must be clean
[ -z "$(git status --porcelain)" ] || { echo "ERROR: working tree is dirty"; exit 1; }
```

### 2. Read current version

Parse the current version from `z/Cargo.toml` under `[workspace.package]`:

```toml
[workspace.package]
version = "X.Y.Z"
```

### 3. Compute new version

Apply the requested semver bump to `X.Y.Z`:

| Bump    | Result          |
|---------|-----------------|
| `patch` | `X.Y.(Z+1)`    |
| `minor` | `X.(Y+1).0`    |
| `major` | `(X+1).0.0`    |

### 4. Update version

Edit `z/Cargo.toml` — replace the version string under `[workspace.package]` with the new version.

### 5. Regenerate lockfile

```bash
cd z && cargo check --workspace
```

This updates `z/Cargo.lock` to reflect the new version.

### 6. Commit

```bash
git add z/Cargo.toml z/Cargo.lock
git commit -m "chore: bump version to VERSION"
```

Replace `VERSION` with the new version (e.g., `0.3.0`).

### 7. Tag

```bash
git tag vVERSION
```

Example: `v0.3.0`.

### 8. Push

```bash
git push origin main --follow-tags
```

This triggers the `release.yml` workflow which builds binaries and creates the GitHub Release.

### 9. Confirm

Print a summary:

```
Released v{VERSION}
Tag pushed — GitHub Actions will build binaries and publish the release.
```
