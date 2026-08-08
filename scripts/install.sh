#!/usr/bin/env bash
# Install Dream toolchain (dream, dreamer, dream-lsp) from GitHub Releases.
#
#   curl --proto '=https' --tlsv1.2 -sSf https://sps014.github.io/dream/install.sh | sh
#
# Env:
#   DREAM_VERSION   optional tag without leading v (default: latest release)
#   DREAM_HOME      install prefix (default: ~/.dream)

set -euo pipefail

REPO="${DREAM_REPO:-sps014/dream}"
PREFIX="${DREAM_HOME:-${HOME}/.dream}"
BIN_DIR="${PREFIX}/bin"
TMPDIR="${TMPDIR:-/tmp}"
WORK="$(mktemp -d "${TMPDIR%/}/dream-install.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: need '$1' on PATH" >&2
    exit 1
  }
}

need curl
need tar
need uname

detect_target() {
  local os arch
  case "$(uname -s)" in
    Linux*) os=linux ;;
    Darwin*) os=macos ;;
    MINGW*|MSYS*|CYGWIN*) os=windows ;;
    *)
      echo "error: unsupported OS: $(uname -s)" >&2
      exit 1
      ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch=x64 ;;
    arm64|aarch64) arch=arm64 ;;
    *)
      echo "error: unsupported arch: $(uname -m)" >&2
      exit 1
      ;;
  esac
  echo "${os}-${arch}"
}

latest_tag() {
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
    | head -n1
}

TARGET="$(detect_target)"
if [ -n "${DREAM_VERSION:-}" ]; then
  TAG="v${DREAM_VERSION#v}"
else
  TAG="$(latest_tag || true)"
fi

if [ -z "${TAG}" ]; then
  cat >&2 <<EOF
error: no Dream release found on https://github.com/${REPO}/releases

Publish a tagged release (v0.1.0+) with toolchain archives, or build from source:

  git clone https://github.com/${REPO}.git
  cd dream
  source ./use-toolchain.sh
EOF
  exit 1
fi

VERSION="${TAG#v}"
if [ "$TARGET" = "windows-x64" ] || [ "$TARGET" = "windows-arm64" ]; then
  ARCHIVE="dream-${VERSION}-${TARGET}.zip"
  EXTRACT=zip
else
  ARCHIVE="dream-${VERSION}-${TARGET}.tar.gz"
  EXTRACT=tar
fi

BASE="https://github.com/${REPO}/releases/download/${TAG}"
URL="${BASE}/${ARCHIVE}"
SUM_URL="${BASE}/SHA256SUMS"

echo "Installing Dream ${VERSION} (${TARGET}) → ${PREFIX}"
echo "Downloading ${URL}"

if ! curl -fsSL "$URL" -o "${WORK}/${ARCHIVE}"; then
  cat >&2 <<EOF
error: failed to download ${URL}

Is ${TAG} published with asset ${ARCHIVE}?
See https://github.com/${REPO}/releases
EOF
  exit 1
fi

if curl -fsSL "$SUM_URL" -o "${WORK}/SHA256SUMS" 2>/dev/null; then
  EXPECTED="$(grep -E "  ${ARCHIVE}\$" "${WORK}/SHA256SUMS" | awk '{print $1}' || true)"
  if [ -n "$EXPECTED" ]; then
    if command -v shasum >/dev/null 2>&1; then
      ACTUAL="$(shasum -a 256 "${WORK}/${ARCHIVE}" | awk '{print $1}')"
    elif command -v sha256sum >/dev/null 2>&1; then
      ACTUAL="$(sha256sum "${WORK}/${ARCHIVE}" | awk '{print $1}')"
    else
      echo "warning: no shasum/sha256sum; skipping checksum verify" >&2
      ACTUAL=""
    fi
    if [ -n "$ACTUAL" ] && [ "$ACTUAL" != "$EXPECTED" ]; then
      echo "error: checksum mismatch for ${ARCHIVE}" >&2
      echo "  expected ${EXPECTED}" >&2
      echo "  got      ${ACTUAL}" >&2
      exit 1
    fi
    if [ -n "$ACTUAL" ]; then
      echo "Checksum OK"
    fi
  fi
fi

mkdir -p "${WORK}/out" "$BIN_DIR"
if [ "$EXTRACT" = tar ]; then
  tar -xzf "${WORK}/${ARCHIVE}" -C "${WORK}/out"
else
  need unzip
  unzip -q "${WORK}/${ARCHIVE}" -d "${WORK}/out"
fi

# Archives contain binaries at top level or in a single directory.
find "${WORK}/out" -type f \( -name 'dream' -o -name 'dream.exe' -o -name 'dreamer' -o -name 'dreamer.exe' -o -name 'dream-lsp' -o -name 'dream-lsp.exe' \) \
  -exec cp -f {} "$BIN_DIR/" \;

chmod +x "${BIN_DIR}/dream" "${BIN_DIR}/dreamer" "${BIN_DIR}/dream-lsp" 2>/dev/null || true

EXT=
case "$TARGET" in
  windows-*) EXT=.exe ;;
esac

cat > "${PREFIX}/toolchain.env" <<EOF
DREAM_HOME=${BIN_DIR}
DREAMER_HOME=${BIN_DIR}
DREAM_BIN=${BIN_DIR}/dream${EXT}
EOF

cat > "${PREFIX}/env.sh" <<EOF
# Dream toolchain
export DREAM_HOME="${BIN_DIR}"
export DREAMER_HOME="${BIN_DIR}"
export DREAM_BIN="${BIN_DIR}/dream${EXT}"
case ":\$PATH:" in
  *":${BIN_DIR}:"*) ;;
  *) export PATH="${BIN_DIR}:\$PATH" ;;
esac
EOF

MARKER="# Dream toolchain (install.sh)"
add_rc_hook() {
  local rc="$1"
  [ -f "$rc" ] || touch "$rc"
  if grep -Fq "$MARKER" "$rc" 2>/dev/null; then
    return 0
  fi
  printf '\n%s\nsource "%s/env.sh"\n' "$MARKER" "$PREFIX" >>"$rc"
  echo "Added PATH hook to ${rc}"
}

case "${SHELL:-}" in
  */zsh) add_rc_hook "${HOME}/.zshrc" ;;
  */bash) add_rc_hook "${HOME}/.bashrc" ;;
  *)
    add_rc_hook "${HOME}/.zshrc"
    add_rc_hook "${HOME}/.bashrc"
    ;;
esac

# shellcheck disable=SC1090
# Current shell:
export DREAM_HOME="${BIN_DIR}"
export DREAMER_HOME="${BIN_DIR}"
export DREAM_BIN="${BIN_DIR}/dream${EXT}"
case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *) export PATH="${BIN_DIR}:${PATH}" ;;
esac

echo
echo "Installed:"
echo "  ${BIN_DIR}/dream${EXT}"
echo "  ${BIN_DIR}/dreamer${EXT}"
echo "  ${BIN_DIR}/dream-lsp${EXT}"
echo
echo "Open a new terminal (or: source ${PREFIX}/env.sh), then:"
echo "  dream --help"
echo "  dreamer --help"
echo "  dreamer init hello && cd hello && dreamer run"
