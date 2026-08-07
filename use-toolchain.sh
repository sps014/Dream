#!/usr/bin/env bash
# Build Dream toolchain binaries and install them for shell + IDE use.
#
#   source ./use-toolchain.sh              # release (default)
#   source ./use-toolchain.sh --debug      # target/debug instead
#   source ./use-toolchain.sh --skip-build # only re-link / re-export
#   source ./use-toolchain.sh --unlink     # remove links, env files, and shell-rc hooks
#
# After install:
#   - `dream`, `dreamer`, `dream-lsp` work from any directory (via ~/.dream/bin + shell rc)
#   - Cursor/VS Code pick up paths from ~/.dream/toolchain.env (reload the window)

# Resolve this script's path when sourced from bash or zsh.
if [ -n "${ZSH_VERSION:-}" ]; then
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

_dream_marker="# Dream toolchain (use-toolchain.sh)"

_dream_cleanup_locals() {
  unset _dream_script _dream_root _dream_sourced _dream_profile _dream_skip_build _dream_unlink _arg
  unset _dream_home _dream_ext _dream_bin _dreamer_bin _dream_lsp_bin _need
  unset _dream_user_dir _dream_env_file _dream_bin_dir _dream_sh_file _dream_rc _dream_marker
  unset _dream_tmp _dream_new_path _dream_p _dream_name
  unset -f _dream_fail _dream_cleanup_locals _dream_remove_rc_hook _dream_strip_path 2>/dev/null || true
}

_dream_fail() {
  _dream_cleanup_locals
  if [ "${_dream_sourced:-0}" -eq 1 ]; then
    return 1
  fi
  exit 1
}

_dream_remove_rc_hook() {
  _dream_rc="$1"
  [ -f "$_dream_rc" ] || return 0
  grep -Fq "$_dream_marker" "$_dream_rc" 2>/dev/null || return 0
  _dream_tmp="$(mktemp)"
  # Drop the marker line and the following `source ~/.dream/env.sh` line.
  awk -v m="$_dream_marker" '
    $0 == m { skip=1; next }
    skip == 1 { skip=0; next }
    { print }
  ' "$_dream_rc" > "$_dream_tmp" && mv "$_dream_tmp" "$_dream_rc"
  echo "Removed Dream PATH hook from ${_dream_rc}"
}

_dream_strip_path() {
  _dream_bin_dir="$1"
  _dream_new_path=""
  _dream_old_ifs="$IFS"
  IFS=':'
  # shellcheck disable=SC2086
  for _dream_p in $PATH; do
    [ "$_dream_p" = "$_dream_bin_dir" ] && continue
    if [ -z "$_dream_new_path" ]; then
      _dream_new_path="$_dream_p"
    else
      _dream_new_path="${_dream_new_path}:${_dream_p}"
    fi
  done
  IFS="$_dream_old_ifs"
  export PATH="$_dream_new_path"
  unset _dream_old_ifs
}

_dream_profile=release
_dream_skip_build=0
_dream_unlink=0
for _arg in "$@"; do
  case "$_arg" in
    --debug) _dream_profile=debug ;;
    --release) _dream_profile=release ;;
    --skip-build) _dream_skip_build=1 ;;
    --unlink|--unsource) _dream_unlink=1 ;;
    -h|--help)
      cat <<'EOF'
Usage: source ./use-toolchain.sh [--release|--debug] [--skip-build]
       source ./use-toolchain.sh --unlink

  --release      Use target/release (default)
  --debug        Use target/debug
  --skip-build   Do not run cargo; only re-link and export
  --unlink       Remove ~/.dream/bin links, toolchain env files, and shell-rc hooks
                 (alias: --unsource). Unsets DREAM_* in this shell when sourced.

Installs symlinks in ~/.dream/bin, writes ~/.dream/toolchain.env (IDE) and
~/.dream/env.sh, and adds `source ~/.dream/env.sh` to ~/.zshrc / ~/.bashrc so
`dream` / `dreamer` / `dream-lsp` work from any directory in new terminals.
EOF
      _dream_cleanup_locals
      if [ "$_dream_sourced" -eq 1 ] 2>/dev/null; then
        return 0
      fi
      exit 0
      ;;
    *)
      echo "unknown option: $_arg (try --help)" >&2
      _dream_fail
      return 1 2>/dev/null || exit 1
      ;;
  esac
done

if [ "$_dream_unlink" -eq 1 ]; then
  if [ -z "${HOME:-}" ]; then
    echo "error: HOME is unset" >&2
    _dream_fail
    return 1 2>/dev/null || exit 1
  fi

  _dream_user_dir="${HOME}/.dream"
  _dream_bin_dir="${_dream_user_dir}/bin"
  _dream_env_file="${_dream_user_dir}/toolchain.env"
  _dream_sh_file="${_dream_user_dir}/env.sh"

  for _dream_name in dream dreamer dream-lsp dream.exe dreamer.exe dream-lsp.exe; do
    if [ -L "${_dream_bin_dir}/${_dream_name}" ] || [ -e "${_dream_bin_dir}/${_dream_name}" ]; then
      rm -f "${_dream_bin_dir}/${_dream_name}"
      echo "Removed ${_dream_bin_dir}/${_dream_name}"
    fi
  done
  rmdir "$_dream_bin_dir" 2>/dev/null || true

  for _dream_f in "$_dream_env_file" "$_dream_sh_file"; do
    if [ -f "$_dream_f" ]; then
      rm -f "$_dream_f"
      echo "Removed ${_dream_f}"
    fi
  done
  rmdir "$_dream_user_dir" 2>/dev/null || true

  for _dream_rc in "${HOME}/.zshrc" "${HOME}/.bashrc"; do
    _dream_remove_rc_hook "$_dream_rc"
  done

  if [ "$_dream_sourced" -eq 1 ]; then
    _dream_strip_path "$_dream_bin_dir"
    unset DREAM_HOME DREAMER_HOME DREAM_BIN
    echo "Unset DREAM_HOME / DREAMER_HOME / DREAM_BIN and removed ~/.dream/bin from PATH in this shell."
  fi

  echo "Dream toolchain unlinked. Open a new terminal (or reload the IDE) so nothing still sees the old paths."
  _dream_cleanup_locals
  if [ "${_dream_sourced:-0}" -eq 1 ]; then
    return 0
  fi
  exit 0
fi

_dream_home="${_dream_root}/target/${_dream_profile}"

if [ "$_dream_skip_build" -eq 0 ]; then
  echo "Building ${_dream_profile} toolchain (dream, dream-lsp, dreamer)..."
  if [ "$_dream_profile" = release ]; then
    (cd "$_dream_root" && cargo build --release -p dream -p dream-lsp -p dreamer) || {
      _dream_fail
      return 1 2>/dev/null || exit 1
    }
  else
    (cd "$_dream_root" && cargo build -p dream -p dream-lsp -p dreamer) || {
      _dream_fail
      return 1 2>/dev/null || exit 1
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
    _dream_fail
    return 1 2>/dev/null || exit 1
  fi
done

if [ -z "${HOME:-}" ]; then
  echo "error: HOME is unset; cannot install user toolchain links" >&2
  _dream_fail
  return 1 2>/dev/null || exit 1
fi

_dream_user_dir="${HOME}/.dream"
_dream_bin_dir="${_dream_user_dir}/bin"
_dream_env_file="${_dream_user_dir}/toolchain.env"
_dream_sh_file="${_dream_user_dir}/env.sh"

mkdir -p "$_dream_bin_dir"

# Stable symlinks so PATH can point at ~/.dream/bin even after rebuilds (re-run this script).
ln -sfn "$_dream_bin" "${_dream_bin_dir}/dream${_dream_ext}"
ln -sfn "$_dreamer_bin" "${_dream_bin_dir}/dreamer${_dream_ext}"
ln -sfn "$_dream_lsp_bin" "${_dream_bin_dir}/dream-lsp${_dream_ext}"
echo "Linked ${_dream_bin_dir}/{dream,dreamer,dream-lsp} -> ${_dream_home}/"

export DREAM_HOME="$_dream_home"
export DREAMER_HOME="$_dream_home"
export DREAM_BIN="$_dream_bin"

# Prefer the stable bin dir on PATH (works the same after rebuild + re-link).
case ":${PATH}:" in
  *":${_dream_bin_dir}:"*) ;;
  *) export PATH="${_dream_bin_dir}:${PATH}" ;;
esac

cat > "$_dream_env_file" <<EOF
# Written by use-toolchain.sh — read by the VS Code/Cursor Dream extension and dreamer.
DREAM_HOME=${DREAM_HOME}
DREAMER_HOME=${DREAMER_HOME}
DREAM_BIN=${DREAM_BIN}
EOF
echo "Wrote ${_dream_env_file}"

cat > "$_dream_sh_file" <<EOF
# Written by use-toolchain.sh — source from ~/.zshrc / ~/.bashrc.
${_dream_marker}
export DREAM_HOME="${DREAM_HOME}"
export DREAMER_HOME="${DREAMER_HOME}"
export DREAM_BIN="${DREAM_BIN}"
case ":\${PATH}:" in
  *":${_dream_bin_dir}:"*) ;;
  *) export PATH="${_dream_bin_dir}:\${PATH}" ;;
esac
EOF
echo "Wrote ${_dream_sh_file}"

# Idempotent shell-rc install so new terminals get dreamer anywhere.
for _dream_rc in "${HOME}/.zshrc" "${HOME}/.bashrc"; do
  if [ -f "$_dream_rc" ] && grep -Fq "$_dream_marker" "$_dream_rc" 2>/dev/null; then
    continue
  fi
  {
    echo ""
    echo "${_dream_marker}"
    echo '[ -f "$HOME/.dream/env.sh" ] && . "$HOME/.dream/env.sh"'
  } >> "$_dream_rc"
  echo "Added Dream PATH hook to ${_dream_rc}"
done

echo "DREAM_HOME=${DREAM_HOME}"
echo "DREAMER_HOME=${DREAMER_HOME}"
echo "DREAM_BIN=${DREAM_BIN}"
echo "Ready: dream=$(command -v dream)  dreamer=$(command -v dreamer)  dream-lsp=$(command -v dream-lsp)"
echo "New terminals pick this up automatically. This shell is already configured if you sourced the script."
echo "Reload the Cursor/VS Code window if the LSP was already running."
echo "To remove:  source ./use-toolchain.sh --unlink"

if [ "$_dream_sourced" -eq 0 ]; then
  echo >&2
  echo "warning: script was executed, not sourced — PATH only updated in this subprocess." >&2
  echo "IDE + ~/.dream/bin + shell rc were still installed." >&2
  echo "For this terminal:  source ./use-toolchain.sh   (or: source ~/.dream/env.sh)" >&2
fi

_dream_cleanup_locals
