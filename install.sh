#!/usr/bin/env bash
# Install script for z — TUI project manager for Zellij.
# Usage: curl -fsSL https://raw.githubusercontent.com/arkan/z/main/install.sh | bash
set -euo pipefail

REPO="arkan/z"
BINARY="z"
INSTALL_DIR="${Z_INSTALL_DIR:-${HOME}/.local/bin}"

# ── helpers ──────────────────────────────────────────────────────────────────

info()  { printf '\033[1;34m%s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m%s\033[0m\n' "$*"; }
err()   { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but not found"
}

# ── detect platform ─────────────────────────────────────────────────────────

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *)      err "unsupported OS: $os" ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *)             err "unsupported architecture: $arch" ;;
  esac

  echo "${arch}-${os}"
}

# ── resolve version ─────────────────────────────────────────────────────────

resolve_version() {
  local version="${Z_VERSION:-latest}"
  if [ "$version" = "latest" ]; then
    need curl
    version="$(curl -fsSL -H "Accept: application/json" \
      "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
    [ -n "$version" ] || err "could not determine latest release"
  fi
  echo "$version"
}

# ── download & verify ───────────────────────────────────────────────────────

download_and_install() {
  local version="$1" target="$2"
  local tag_version="${version#v}"
  local archive="z-v${tag_version}-${target}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/${version}/${archive}"
  local checksums_url="https://github.com/${REPO}/releases/download/${version}/checksums-sha256.txt"

  local tmpdir
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  info "downloading ${archive}…"
  curl -fsSL -o "${tmpdir}/${archive}" "$url" \
    || err "download failed — does release ${version} exist for ${target}?"

  # verify checksum if sha256sum or shasum is available
  if command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1; then
    info "verifying checksum…"
    curl -fsSL -o "${tmpdir}/checksums-sha256.txt" "$checksums_url" 2>/dev/null || true
    if [ -f "${tmpdir}/checksums-sha256.txt" ]; then
      local expected
      expected="$(grep "${archive}" "${tmpdir}/checksums-sha256.txt" | awk '{print $1}')"
      if [ -n "$expected" ]; then
        local actual
        if command -v sha256sum >/dev/null 2>&1; then
          actual="$(sha256sum "${tmpdir}/${archive}" | awk '{print $1}')"
        else
          actual="$(shasum -a 256 "${tmpdir}/${archive}" | awk '{print $1}')"
        fi
        [ "$actual" = "$expected" ] || err "checksum mismatch (expected ${expected}, got ${actual})"
        ok "checksum verified ✓"
      fi
    fi
  fi

  info "extracting to ${INSTALL_DIR}…"
  mkdir -p "$INSTALL_DIR"
  tar xzf "${tmpdir}/${archive}" -C "$tmpdir"
  install -m 755 "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"

  ok "installed ${BINARY} ${version} to ${INSTALL_DIR}/${BINARY}"
}

# ── PATH check ──────────────────────────────────────────────────────────────

check_path() {
  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      printf '\n\033[1;33mwarning:\033[0m %s is not in your PATH.\n' "$INSTALL_DIR"
      echo "Add it with:"
      echo ""
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo ""
      ;;
  esac
}

# ── main ─────────────────────────────────────────────────────────────────────

main() {
  need curl
  need tar

  local target version
  target="$(detect_target)"
  version="$(resolve_version)"

  info "platform: ${target}"
  info "version:  ${version}"

  download_and_install "$version" "$target"
  check_path
}

main
