#!/bin/sh
# Portable opt-in Cargo cache topology for Codewhale worktrees.
#
# Isolated per-workspace intermediates (Cargo build-dir + {workspace-path-hash})
# plus optional shared sccache. Never hard-codes a machine path. Does not
# change product behavior. This file is safe to source from other /bin/sh
# helpers; the CLI path is the only one that calls `set -eu` and `exit`.
#
# Usage:
#   . scripts/dev-cache.sh && codewhale_dev_cache_apply
#   scripts/dev-cache.sh --status
#   scripts/dev-cache.sh --self-check
#   scripts/dev-cache.sh --print-exports
#   scripts/dev-cache.sh --help
#
# Environment:
#   CODEWHALE_CACHE_ROOT        cache root (default:
#                               ${XDG_CACHE_HOME:-$HOME/.cache}/codewhale)
#   CODEWHALE_DEV_CACHE         auto|1|force|local|0   (default auto)
#                               auto/1/force = isolated build-dir (still
#                                       respects an already-set
#                                       CARGO_TARGET_DIR / CARGO_BUILD_BUILD_DIR)
#                               local = keep compiling into ./target
#                               0     = do nothing
#   CODEWHALE_SCCACHE           auto|1|0         (default auto)
#                               auto  = wrap rustc only when incremental is
#                                       already off and sccache is on PATH
#                               1     = request CARGO_INCREMENTAL=0 (unless
#                                       the caller already set it to on) and
#                                       wrap if sccache exists
#                               0     = never wrap
#   CODEWHALE_DEV_CACHE_QUIET   1 to suppress the one-line status
#   CODEWHALE_DEV_CACHE_REPO_ROOT
#                               workspace root used for ./target detection
#                               (default: $PWD)
#   CODEWHALE_RUSTC_COMMIT      override the rustc commit used to namespace
#                               SCCACHE_DIR (tests / offline hosts)
#
# sccache cannot cache incremental units. This helper never enables the
# wrapper while CARGO_INCREMENTAL is on, and it never turns incremental off
# unless CODEWHALE_SCCACHE=1. Everyday edits stay on cargo's incremental
# default. New-worktree / CI-like rebuilds that already set
# CARGO_INCREMENTAL=0 get the measured shared-compiler-output path.

codewhale_dev_cache_truthy() {
  case ${1:-} in
    1|true|yes|on|force) return 0 ;;
    *) return 1 ;;
  esac
}

codewhale_dev_cache_falsey() {
  case ${1:-} in
    0|false|no|off|never|disabled) return 0 ;;
    *) return 1 ;;
  esac
}

codewhale_dev_cache_root() {
  if [ -n "${CODEWHALE_CACHE_ROOT:-}" ]; then
    printf '%s\n' "$CODEWHALE_CACHE_ROOT"
    return
  fi
  if [ -n "${XDG_CACHE_HOME:-}" ]; then
    printf '%s\n' "$XDG_CACHE_HOME/codewhale"
    return
  fi
  if [ -n "${HOME:-}" ]; then
    printf '%s\n' "$HOME/.cache/codewhale"
    return
  fi
  printf '%s\n' "${TMPDIR:-/tmp}/codewhale-cache"
}

codewhale_dev_cache_repo_root() {
  if [ -n "${CODEWHALE_DEV_CACHE_REPO_ROOT:-}" ]; then
    printf '%s\n' "$CODEWHALE_DEV_CACHE_REPO_ROOT"
    return
  fi
  pwd
}

codewhale_dev_cache_rustc_commit() {
  if [ -n "${CODEWHALE_RUSTC_COMMIT:-}" ]; then
    printf '%s\n' "$CODEWHALE_RUSTC_COMMIT"
    return
  fi
  # rustc -vV is cheap and does not start a compile.
  _cw_out=$(rustc -vV 2>/dev/null || true)
  _cw_commit=$(printf '%s\n' "$_cw_out" | awk '/^commit-hash:/{print $2; exit}')
  if [ -n "$_cw_commit" ]; then
    printf '%s\n' "$_cw_commit"
  else
    printf '%s\n' "unknown"
  fi
}

codewhale_dev_cache_cargo_version_mm() {
  # cargo 1.97.0 -> 1097. Empty if cargo is missing or unparsable.
  cargo --version 2>/dev/null | awk '{
    split($2, a, ".")
    if (a[1] ~ /^[0-9]+$/ && a[2] ~ /^[0-9]+$/)
      printf "%d\n", a[1]*1000+a[2]
  }'
}

# Stable 16-hex-char fingerprint of a path. Used only for the pre-1.91
# CARGO_TARGET_DIR fallback; Cargo 1.91+ expands {workspace-path-hash}.
codewhale_dev_cache_path_fingerprint() {
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "$1" | sha256sum | awk '{print substr($1,1,16)}'
  elif command -v shasum >/dev/null 2>&1; then
    printf '%s' "$1" | shasum -a 256 | awk '{print substr($1,1,16)}'
  elif command -v openssl >/dev/null 2>&1; then
    printf '%s' "$1" | openssl dgst -sha256 | awk '{print substr($NF,1,16)}'
  else
    printf '%s' "$1" | cksum | awk '{printf "%s%08s\n", $1, $2}' | tr ' ' '0'
  fi
}

codewhale_dev_cache_incremental_off() {
  case ${CARGO_INCREMENTAL:-} in
    0|false|no|off) return 0 ;;
    *) return 1 ;;
  esac
}

codewhale_dev_cache_incremental_on() {
  case ${CARGO_INCREMENTAL:-} in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

codewhale_dev_cache_sccache_bin() {
  command -v sccache 2>/dev/null || true
}

codewhale_dev_cache_log() {
  if [ "${CODEWHALE_DEV_CACHE_QUIET:-}" = 1 ]; then
    return 0
  fi
  printf 'dev-cache: %s\n' "$*" >&2
}

# Apply the topology to the current shell. Idempotent. Never overrides an
# already-set CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR, RUSTC_WRAPPER, or
# SCCACHE_DIR.
codewhale_dev_cache_apply() {
  CODEWHALE_DEV_CACHE_MODE=${CODEWHALE_DEV_CACHE_MODE:-}
  # Re-entry after a previous apply in this shell: keep the first decision
  # unless the caller unset the mode (tests do that between cases).
  if [ -n "${CODEWHALE_DEV_CACHE_APPLIED:-}" ] && [ -n "${CODEWHALE_DEV_CACHE_MODE:-}" ]; then
    return 0
  fi

  _cw_want=${CODEWHALE_DEV_CACHE:-auto}
  _cw_root=$(codewhale_dev_cache_root)
  _cw_sccache_want=${CODEWHALE_SCCACHE:-auto}
  _cw_commit=$(codewhale_dev_cache_rustc_commit)
  CODEWHALE_DEV_CACHE_SCCACHE_DIR=${SCCACHE_DIR:-${_cw_root}/sccache/${_cw_commit}}
  CODEWHALE_DEV_CACHE_BUILD_DIR=${_cw_root}/build/'{workspace-path-hash}'
  CODEWHALE_DEV_CACHE_LEGACY_TARGET_DIR=${_cw_root}/target/$(codewhale_dev_cache_path_fingerprint "$(codewhale_dev_cache_repo_root)")

  if codewhale_dev_cache_falsey "$_cw_want"; then
    CODEWHALE_DEV_CACHE_MODE=disabled
    CODEWHALE_DEV_CACHE_SCCACHE=disabled
    CODEWHALE_DEV_CACHE_APPLIED=1
    export CODEWHALE_DEV_CACHE_MODE CODEWHALE_DEV_CACHE_SCCACHE
    codewhale_dev_cache_log "disabled (CODEWHALE_DEV_CACHE=${_cw_want})"
    return 0
  fi

  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    CODEWHALE_DEV_CACHE_MODE=inherited-target-dir
  elif [ -n "${CARGO_BUILD_BUILD_DIR:-}" ]; then
    CODEWHALE_DEV_CACHE_MODE=inherited-build-dir
  elif [ "$_cw_want" = local ]; then
    # Explicit stay-in-./target. A stub target/ created by a previous
    # isolated build-dir run is not a reason to abandon isolation: Cargo
    # still writes CACHEDIR.TAG (and sometimes finals) under ./target
    # when build-dir is split, and treating that as "warm" made the
    # second command recompile everything into a second tree.
    CODEWHALE_DEV_CACHE_MODE=existing-target
  else
    CODEWHALE_DEV_CACHE_MODE=isolated-build-dir
  fi

  case $CODEWHALE_DEV_CACHE_MODE in
    force-isolated|isolated-build-dir)
      _cw_mm=$(codewhale_dev_cache_cargo_version_mm)
      if [ -n "$_cw_mm" ] && [ "$_cw_mm" -lt 1091 ]; then
        # Cargo 1.91 introduced build.build-dir. Older cargo only isolates
        # via CARGO_TARGET_DIR; keep that fallback per-workspace.
        if [ -z "${CARGO_TARGET_DIR:-}" ]; then
          CARGO_TARGET_DIR=$CODEWHALE_DEV_CACHE_LEGACY_TARGET_DIR
          export CARGO_TARGET_DIR
        fi
        CODEWHALE_DEV_CACHE_MODE=${CODEWHALE_DEV_CACHE_MODE}-legacy-target-dir
      else
        CARGO_BUILD_BUILD_DIR=$CODEWHALE_DEV_CACHE_BUILD_DIR
        export CARGO_BUILD_BUILD_DIR
        mkdir -p "$_cw_root/build" 2>/dev/null || true
      fi
      ;;
  esac

  # sccache: never wrap incremental rustc. Missing binary is a fallback,
  # not an error.
  if codewhale_dev_cache_falsey "$_cw_sccache_want"; then
    CODEWHALE_DEV_CACHE_SCCACHE=disabled
  elif [ -n "${RUSTC_WRAPPER:-}" ]; then
    case $RUSTC_WRAPPER in
      *sccache*) CODEWHALE_DEV_CACHE_SCCACHE=external-sccache ;;
      *) CODEWHALE_DEV_CACHE_SCCACHE=external-wrapper ;;
    esac
  else
    if codewhale_dev_cache_truthy "$_cw_sccache_want" && ! codewhale_dev_cache_incremental_on; then
      # Request the measured new-worktree / CI-like topology.
      if [ -z "${CARGO_INCREMENTAL:-}" ]; then
        CARGO_INCREMENTAL=0
        export CARGO_INCREMENTAL
      fi
    fi
    if ! codewhale_dev_cache_incremental_off; then
      CODEWHALE_DEV_CACHE_SCCACHE=skipped-incremental
    else
      _cw_bin=$(codewhale_dev_cache_sccache_bin)
      if [ -z "$_cw_bin" ]; then
        CODEWHALE_DEV_CACHE_SCCACHE=not-found
      else
        RUSTC_WRAPPER=$_cw_bin
        export RUSTC_WRAPPER
        if [ -z "${SCCACHE_DIR:-}" ]; then
          SCCACHE_DIR=$CODEWHALE_DEV_CACHE_SCCACHE_DIR
          export SCCACHE_DIR
        fi
        mkdir -p "$SCCACHE_DIR" 2>/dev/null || true
        CODEWHALE_DEV_CACHE_SCCACHE=enabled
      fi
    fi
  fi

  CODEWHALE_DEV_CACHE_APPLIED=1
  export CODEWHALE_DEV_CACHE_MODE CODEWHALE_DEV_CACHE_SCCACHE
  export CODEWHALE_DEV_CACHE_SCCACHE_DIR

  _cw_line="mode=${CODEWHALE_DEV_CACHE_MODE} sccache=${CODEWHALE_DEV_CACHE_SCCACHE}"
  case $CODEWHALE_DEV_CACHE_MODE in
    isolated-build-dir*|force-isolated*)
      _cw_line="${_cw_line} build-dir=${CARGO_BUILD_BUILD_DIR:-${CARGO_TARGET_DIR:-unset}}"
      ;;
    inherited-target-dir)
      _cw_line="${_cw_line} CARGO_TARGET_DIR=${CARGO_TARGET_DIR}"
      ;;
    inherited-build-dir)
      _cw_line="${_cw_line} CARGO_BUILD_BUILD_DIR=${CARGO_BUILD_BUILD_DIR}"
      ;;
    existing-target)
      _cw_line="${_cw_line} using ./target (CODEWHALE_DEV_CACHE=local)"
      ;;
  esac
  if [ "${CODEWHALE_DEV_CACHE_SCCACHE}" = enabled ]; then
    _cw_line="${_cw_line} SCCACHE_DIR=${SCCACHE_DIR} rustc=${_cw_commit}"
  fi
  codewhale_dev_cache_log "$_cw_line"
}

codewhale_dev_cache_status() {
  _cw_tmpl=${CODEWHALE_DEV_CACHE_BUILD_DIR:-}
  if [ -z "$_cw_tmpl" ]; then
    _cw_tmpl=$(codewhale_dev_cache_root)/build/'{workspace-path-hash}'
  fi
  printf '%s\n' \
    "cache_root=$(codewhale_dev_cache_root)" \
    "repo_root=$(codewhale_dev_cache_repo_root)" \
    "mode=${CODEWHALE_DEV_CACHE_MODE:-unset}" \
    "sccache=${CODEWHALE_DEV_CACHE_SCCACHE:-unset}" \
    "build_dir_template=${_cw_tmpl}" \
    "CARGO_BUILD_BUILD_DIR=${CARGO_BUILD_BUILD_DIR:-<unset>}" \
    "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>}" \
    "CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-<unset>}" \
    "RUSTC_WRAPPER=${RUSTC_WRAPPER:-<unset>}" \
    "SCCACHE_DIR=${SCCACHE_DIR:-<unset>}" \
    "rustc_commit=$(codewhale_dev_cache_rustc_commit)" \
    "sccache_bin=$(codewhale_dev_cache_sccache_bin)"
}

# Extra cargo CLI flags that force template expansion of build-dir.
# CARGO_BUILD_BUILD_DIR is still exported for nested tools; --config is
# what Cargo documents as supporting {workspace-path-hash}.
codewhale_dev_cache_exec_cargo() {
  case ${CODEWHALE_DEV_CACHE_MODE:-} in
    isolated-build-dir|force-isolated)
      if [ -n "${CARGO_BUILD_BUILD_DIR:-}" ]; then
        exec cargo --config "build.build-dir = \"${CARGO_BUILD_BUILD_DIR}\"" "$@"
      fi
      ;;
  esac
  exec cargo "$@"
}

codewhale_dev_cache_print_exports() {
  [ -n "${CARGO_BUILD_BUILD_DIR:-}" ] && printf 'export CARGO_BUILD_BUILD_DIR=%s\n' "$CARGO_BUILD_BUILD_DIR"
  [ -n "${CARGO_TARGET_DIR:-}" ] && printf 'export CARGO_TARGET_DIR=%s\n' "$CARGO_TARGET_DIR"
  [ -n "${CARGO_INCREMENTAL:-}" ] && printf 'export CARGO_INCREMENTAL=%s\n' "$CARGO_INCREMENTAL"
  [ -n "${RUSTC_WRAPPER:-}" ] && printf 'export RUSTC_WRAPPER=%s\n' "$RUSTC_WRAPPER"
  [ -n "${SCCACHE_DIR:-}" ] && printf 'export SCCACHE_DIR=%s\n' "$SCCACHE_DIR"
  [ -n "${CODEWHALE_DEV_CACHE_MODE:-}" ] && printf 'export CODEWHALE_DEV_CACHE_MODE=%s\n' "$CODEWHALE_DEV_CACHE_MODE"
  [ -n "${CODEWHALE_DEV_CACHE_SCCACHE:-}" ] && printf 'export CODEWHALE_DEV_CACHE_SCCACHE=%s\n' "$CODEWHALE_DEV_CACHE_SCCACHE"
}

codewhale_dev_cache_self_check() {
  _cw_fail=0
  _cw_root=$(codewhale_dev_cache_root)
  _cw_home=${HOME:-}
  _cw_xdg=${XDG_CACHE_HOME:-}

  if [ -z "${CODEWHALE_CACHE_ROOT:-}" ]; then
    if [ -n "$_cw_xdg" ]; then
      case $_cw_root in
        "$_cw_xdg"/codewhale) ;;
        *)
          printf 'self-check: expected XDG default %s/codewhale, got %s\n' "$_cw_xdg" "$_cw_root" >&2
          _cw_fail=1
          ;;
      esac
    elif [ -n "$_cw_home" ]; then
      case $_cw_root in
        "$_cw_home"/.cache/codewhale) ;;
        *)
          printf 'self-check: expected HOME default %s/.cache/codewhale, got %s\n' "$_cw_home" "$_cw_root" >&2
          _cw_fail=1
          ;;
      esac
    fi
  fi

  case ${CARGO_BUILD_BUILD_DIR:-} in
    *'{workspace-path-hash}'*)
      ;;
    '')
      case ${CODEWHALE_DEV_CACHE_MODE:-} in
        isolated-build-dir|force-isolated)
          printf 'self-check: isolated mode did not set CARGO_BUILD_BUILD_DIR\n' >&2
          _cw_fail=1
          ;;
      esac
      ;;
    *)
      case ${CODEWHALE_DEV_CACHE_MODE:-} in
        isolated-build-dir|force-isolated)
          printf 'self-check: isolated build-dir lacks {workspace-path-hash}: %s\n' "$CARGO_BUILD_BUILD_DIR" >&2
          _cw_fail=1
          ;;
      esac
      ;;
  esac

  if [ "${CODEWHALE_DEV_CACHE_SCCACHE:-}" = enabled ]; then
    if ! codewhale_dev_cache_incremental_off; then
      printf 'self-check: sccache enabled while incremental is not off\n' >&2
      _cw_fail=1
    fi
    case ${RUSTC_WRAPPER:-} in
      *sccache*) ;;
      *)
        printf 'self-check: sccache enabled but RUSTC_WRAPPER=%s\n' "${RUSTC_WRAPPER:-<unset>}" >&2
        _cw_fail=1
        ;;
    esac
  fi

  if [ "${CODEWHALE_DEV_CACHE_SCCACHE:-}" = not-found ]; then
    if [ -n "${RUSTC_WRAPPER:-}" ]; then
      printf 'self-check: missing sccache must not set RUSTC_WRAPPER (%s)\n' "$RUSTC_WRAPPER" >&2
      _cw_fail=1
    fi
  fi

  if [ "$_cw_fail" -eq 0 ]; then
    printf 'dev-cache: self-check ok\n'
    return 0
  fi
  printf 'dev-cache: self-check FAILED\n' >&2
  return 1
}

codewhale_dev_cache_usage() {
  cat <<'EOF'
usage: scripts/dev-cache.sh --status|--self-check|--print-exports|--help

Portable opt-in Cargo cache topology. Source this file and call
codewhale_dev_cache_apply from scripts/dev-test.sh / scripts/dev-cargo.sh.

  --status          apply (if needed) and print key=value status
  --self-check      apply and check portable defaults / sccache fallback
  --print-exports   apply and print export statements for eval
  --help            this message

Environment overrides are documented at the top of this file. Defaults
never contain a machine-specific absolute path.
EOF
}

# CLI only when this file is the executed program, not when sourced.
case ${0##*/} in
  dev-cache.sh)
    set -eu
    _cw_cmd=${1:---status}
    case $_cw_cmd in
      --status|-s)
        codewhale_dev_cache_apply
        codewhale_dev_cache_status
        ;;
      --self-check)
        codewhale_dev_cache_apply
        codewhale_dev_cache_status
        codewhale_dev_cache_self_check
        ;;
      --print-exports)
        codewhale_dev_cache_apply
        codewhale_dev_cache_print_exports
        ;;
      --help|-h)
        codewhale_dev_cache_usage
        ;;
      *)
        codewhale_dev_cache_usage >&2
        exit 2
        ;;
    esac
    ;;
esac
