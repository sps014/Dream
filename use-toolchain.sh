#!/usr/bin/env bash
# Build Dream toolchain binaries and point DREAM_HOME / DREAMER_HOME / PATH at them.
#
# Must be sourced so exports reach your current shell (bash or zsh):
#   source ./use-toolchain.sh              # release (default)
#   source ./use-toolchain.sh --debug      # target/debug instead
#   source ./use-toolchain.sh --skip-build # only export paths
#
# Then reload the Cursor/VS Code window (or restart the Dream extension).
# The script also writes ~/.dream/toolchain.env so the IDE picks up paths without
# inheriting this shell's environment (GUI apps do not see `export` from a terminal).

# Resolve this script's path when sourced from bash or zsh.
if [ -n "${ZSH_VERSION:-}" ]; then
  # zsh: %x is the file being sourced/executed
  # shellcheck disable=SC2296
  _dream_script="${(%):-%x}"
elif [ -n "${BASH_SOURCE[0]:-}" ]; then
  _dream_script="${BASH_SOURCE[0]}"
else
  _dream_script="$0"
fi

_dream_root="$(cd "$(dirname "$_dream_script")" && pwd)"

_dream_sourced=0
if [ -n "${ZSH_VERSION:-}" ]; then
  case "${ZSH_EVAL_CONTEXT:-}" in *:file:*) _dream_sourced=1 ;; esac
elif [ -n "${BASH_VERSION:-}" ]; then
  (return 0 2>/dev/null) && _dream_sourced=1
fi

_dream_profile=release
_dream_skip_build=0
for _arg in "$@"; do
  case "$_arg" in
    --debug) _dream_profile=debug ;;
    --release) _dream_profile=release ;;
    --skip-build) _dream_skip_build=1 ;;
    -h|--help)
      cat <<'EOF'
Usage: source ./use-toolchain.sh [--release|--debug] [--skip-build]

  --release      Use target/release (default)
  --debug        Use target/debug
  --skip-build   Do not run cargo; only export paths

Exports DREAM_HOME, DREAMER_HOME, DREAM_BIN, prepends that directory to PATH,
and writes ~/.dream/toolchain.env for Cursor/VS Code (GUI apps do not inherit shell exports).
EOF
      unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg
      if [ "$_dream_sourced" -eq 1 ] 2>/dev/null; then
        return 0
      fi
      exit 0
      ;;
    *)
      echo "unknown option: $_arg (try --help)" >&2
      unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg
      if [ "${_dream_sourced:-0}" -eq 1 ]; then
        return 1
      fi
      exit 1
      ;;
  esac
done

_dream_home="${_dream_root}/target/${_dream_profile}"

if [ "$_dream_skip_build" -eq 0 ]; then
  echo "Building ${_dream_profile} toolchain (dream, dream-lsp, dreamer)..."
  if [ "$_dream_profile" = release ]; then
    (cd "$_dream_root" && cargo build --release -p dream -p dream-lsp -p dreamer) || {
      unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg _dream_home
      if [ "${_dream_sourced:-0}" -eq 1 ]; then return 1; fi
      exit 1
    }
  else
    (cd "$_dream_root" && cargo build -p dream -p dream-lsp -p dreamer) || {
      unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg _dream_home
      if [ "${_dream_sourced:-0}" -eq 1 ]; then return 1; fi
      exit 1
    }
  fi
fi

_dream_ext=
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*) _dream_ext=.exe ;;
esac

_dream_bin="${_dream_home}/dream${_dream_ext}"
_dreamer_bin="${_dream_home}/dreamer${_dream_ext}"
_dream_lsp_bin="${_dream_home}/dream-lsp${_dream_ext}"

for _need in "$_dream_bin" "$_dreamer_bin" "$_dream_lsp_bin"; do
  if [ ! -f "$_need" ]; then
    echo "error: missing ${_need}; build failed or omit --skip-build" >&2
    unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg
    unset _dream_home _dream_ext _dream_bin _dreamer_bin _dream_lsp_bin _need
    if [ "${_dream_sourced:-0}" -eq 1 ]; then return 1; fi
    exit 1
  fi
done

export DREAM_HOME="$_dream_home"
export DREAMER_HOME="$_dream_home"
export DREAM_BIN="$_dream_bin"

case ":${PATH}:" in
  *":${_dream_home}:"*) ;;
  *) export PATH="${_dream_home}:${PATH}" ;;
esac

# Persist for GUI editors (Cursor/VS Code do not inherit this shell's exports).
_dream_user_dir="${HOME:-}/.dream"
_dream_env_file="${_dream_user_dir}/toolchain.env"
if [ -n "${HOME:-}" ]; then
  mkdir -p "$_dream_user_dir"
  cat > "$_dream_env_file" <<EOF
# Written by use-toolchain.sh — read by the VS Code/Cursor Dream extension and dreamer.
DREAM_HOME=${DREAM_HOME}
DREAMER_HOME=${DREAMER_HOME}
DREAM_BIN=${DREAM_BIN}
EOF
  echo "Wrote ${_dream_env_file} (picked up by the IDE without relaunching from this shell)"
fi

echo "DREAM_HOME=${DREAM_HOME}"
echo "DREAMER_HOME=${DREAMER_HOME}"
echo "DREAM_BIN=${DREAM_BIN}"
echo "PATH starts with ${_dream_home}"
echo "Ready: dream=$(command -v dream)  dreamer=$(command -v dreamer)  dream-lsp=$(command -v dream-lsp)"
echo "Reload the Dream extension / Cursor window if the LSP was already running."

if [ "$_dream_sourced" -eq 0 ]; then
  echo >&2
  echo "warning: script was executed, not sourced — shell exports only applied to this subprocess." >&2
  echo "(${_dream_env_file} was still written for the IDE.)" >&2
  echo "run:  source ./use-toolchain.sh" >&2
fi

unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _arg
unset _dream_home _dream_ext _dream_bin _dreamer_bin _dream_lsp_bin _need
unset _dream_user_dir _dream_env_file
