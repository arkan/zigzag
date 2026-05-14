#!/usr/bin/env bash
set -euo pipefail

REMOTE="${GIT_REMOTE:-origin}"
DRY_RUN=0
FORCE=0
BUMP_TYPE=""
EXPLICIT_VERSION=""

usage() {
  cat <<'USAGE'
Usage: scripts/create-release.sh [--dry-run] [--force] [patch|minor|major|x.y.z|vx.y.z]

Creates an annotated git tag and pushes it.
GitHub Actions creates the GitHub Release from the pushed tag.
If no bump type is provided, the script asks via fzf.

Options:
  --dry-run  Print the computed version without creating a tag.
  --force    Allow tagging current HEAD when the working tree is dirty.

Environment:
  GIT_REMOTE  Git remote used for tag lookup/pushes. Defaults to origin.
USAGE
}

info() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but not found"
}

parse_args() {
  local arg
  for arg in "$@"; do
    case "$arg" in
      --dry-run)
        DRY_RUN=1
        ;;
      --force)
        FORCE=1
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      patch|minor|major)
        [ -z "$BUMP_TYPE" ] || err "bump type provided more than once"
        [ -z "$EXPLICIT_VERSION" ] || err "bump type cannot be combined with an explicit version"
        BUMP_TYPE="$arg"
        ;;
      v[0-9]*.[0-9]*.[0-9]*|[0-9]*.[0-9]*.[0-9]*)
        [ -z "$BUMP_TYPE" ] || err "explicit version cannot be combined with a bump type"
        [ -z "$EXPLICIT_VERSION" ] || err "explicit version provided more than once"
        EXPLICIT_VERSION="$arg"
        ;;
      *)
        err "unsupported argument: $arg"
        ;;
    esac
  done
}

select_bump_type() {
  if [ -n "$BUMP_TYPE" ]; then
    printf '%s\n' "$BUMP_TYPE"
    return
  fi

  need fzf
  printf 'patch\nminor\nmajor\n' | fzf --prompt="Bump type: " --height=5 --reverse
}

next_version_for() {
  local current_version="$1"
  local bump_type="$2"
  local raw_version major minor patch

  raw_version="${current_version#v}"
  if [[ ! "$raw_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    err "latest tag '$current_version' is not a semantic version tag like v1.2.3"
  fi

  major="${BASH_REMATCH[1]}"
  minor="${BASH_REMATCH[2]}"
  patch="${BASH_REMATCH[3]}"

  case "$bump_type" in
    patch)
      patch=$((patch + 1))
      ;;
    minor)
      minor=$((minor + 1))
      patch=0
      ;;
    major)
      major=$((major + 1))
      minor=0
      patch=0
      ;;
    *)
      err "invalid bump type: $bump_type"
      ;;
  esac

  printf 'v%s.%s.%s\n' "$major" "$minor" "$patch"
}

normalize_explicit_version() {
  local version="$1"
  local raw_version="${version#v}"

  if [[ ! "$raw_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    err "explicit version '$version' must be semantic version x.y.z or vx.y.z"
  fi

  printf 'v%s\n' "$raw_version"
}

ensure_release_ready() {
  local next_version="$1"
  local branch status

  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || err "not inside a git worktree"
  status="$(git status --porcelain)"
  if [ -n "$status" ]; then
    if [ "$FORCE" -ne 1 ]; then
      err "working tree must be clean before creating a release"
    fi
    info "Warning: working tree is dirty; tagging current HEAD because --force was provided."
  fi

  branch="$(git branch --show-current)"
  [ -n "$branch" ] || err "refusing to release from a detached HEAD"

  git remote get-url "$REMOTE" >/dev/null 2>&1 || err "git remote '$REMOTE' does not exist"

  if git rev-parse --verify --quiet "$next_version" >/dev/null; then
    err "local tag '$next_version' already exists"
  fi

  if git ls-remote --exit-code --tags "$REMOTE" "refs/tags/${next_version}" >/dev/null 2>&1; then
    err "remote tag '$next_version' already exists on '$REMOTE'"
  else
    local remote_tag_status=$?
    [ "$remote_tag_status" -eq 2 ] || err "failed to check remote tag '$next_version' on '$REMOTE'"
  fi
}

confirm_apply() {
  local answer
  printf 'Apply? [y/N] '
  IFS= read -r answer || return 1
  [[ "$answer" == [yY] ]]
}

create_release_tag() {
  local next_version="$1"

  git tag -a "$next_version" -m "Release $next_version"
  git push "$REMOTE" "$next_version"

  info "Pushed tag $next_version. GitHub Actions will create the release."
}

main() {
  local current_version bump_type next_version

  parse_args "$@"
  need git

  current_version="$(git describe --tags --abbrev=0)"
  info "Current version: $current_version"

  if [ -n "$EXPLICIT_VERSION" ]; then
    next_version="$(normalize_explicit_version "$EXPLICIT_VERSION")"
  else
    bump_type="$(select_bump_type)" || exit 0
    next_version="$(next_version_for "$current_version" "$bump_type")"
  fi
  info "Next version: $next_version"

  if [ "$DRY_RUN" -eq 1 ]; then
    info "Dry run: would create and push tag $next_version."
    exit 0
  fi

  ensure_release_ready "$next_version"

  if ! confirm_apply; then
    info "Cancelled."
    exit 0
  fi

  create_release_tag "$next_version"
}

main "$@"
