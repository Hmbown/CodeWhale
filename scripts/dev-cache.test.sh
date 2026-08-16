#!/bin/sh
# Hermetic tests for scripts/dev-cache.sh. No cargo compile, no cache
# deletion. Exercises portable defaults, overrides, missing-sccache
# fallback, incremental gating, and new-vs-existing worktree policy.
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
DEV_CACHE=$repo_root/scripts/dev-cache.sh
DEV_CARGO=$repo_root/scripts/dev-cargo.sh

fail=0
pass=0

ok() {
  pass=$((pass + 1))
  printf 'ok   - %s\n' "$1"
}

bad() {
  fail=$((fail + 1))
  printf 'FAIL - %s\n' "$1"
  if [ -n "${2:-}" ]; then
    printf '%s\n' "$2" | sed 's/^/       /'
  fi
}

work=$(mktemp -d "${TMPDIR:-/tmp}/codewhale-dev-cache.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup EXIT INT HUP TERM

HOME_DIR=$work/home
mkdir -p "$HOME_DIR"
FAKEBIN=$work/bin
mkdir -p "$FAKEBIN"
NEW_WT=$work/new-wt
OLD_WT=$work/old-wt
mkdir -p "$NEW_WT" "$OLD_WT/target"

# cargo/rustc/sccache are injected per case. Keep the base PATH hermetic.
BASE_PATH=$FAKEBIN:/usr/bin:/bin

run_apply() {
  env -i \
    PATH="$BASE_PATH" \
    HOME="$HOME_DIR" \
    CODEWHALE_DEV_CACHE_QUIET=1 \
    DEV_CACHE="$DEV_CACHE" \
    "$@" \
    /bin/sh -c '
      set -eu
      . "$DEV_CACHE"
      codewhale_dev_cache_apply
      codewhale_dev_cache_status
    ' # shellcheck disable=SC2016 -- $DEV_CACHE expands in the child.
}

contains() {
  printf '%s' "$1" | grep -qF -- "$2"
}

# 1. Portable default root is $HOME/.cache/codewhale, never a desk path.
out=$(run_apply CODEWHALE_DEV_CACHE=0 CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT")
if contains "$out" "cache_root=$HOME_DIR/.cache/codewhale"; then
  ok "default cache root is HOME/.cache/codewhale"
else
  bad "default cache root is HOME/.cache/codewhale" "$out"
fi
if printf '%s\n' "$out" | grep -v '^repo_root=' | grep -qF /Volumes/VIXinSSD; then
  bad "default paths must not mention /Volumes/VIXinSSD" "$out"
else
  ok "default paths do not mention /Volumes/VIXinSSD"
fi
if grep -E '/Users/hunterbown|/Volumes/VIXinSSD' "$DEV_CACHE" "$DEV_CARGO" >/dev/null; then
  bad "helper source must not hard-code a desk path"
else
  ok "helper source has no desk-local absolute paths"
fi

# 2. XDG_CACHE_HOME and CODEWHALE_CACHE_ROOT win in that documented order.
out=$(run_apply CODEWHALE_DEV_CACHE=0 XDG_CACHE_HOME="$work/xdg")
if contains "$out" "cache_root=$work/xdg/codewhale"; then
  ok "XDG_CACHE_HOME/codewhale is the default when set"
else
  bad "XDG_CACHE_HOME/codewhale is the default when set" "$out"
fi
out=$(run_apply CODEWHALE_DEV_CACHE=0 \
  XDG_CACHE_HOME="$work/xdg" \
  CODEWHALE_CACHE_ROOT="$work/explicit-root")
if contains "$out" "cache_root=$work/explicit-root"; then
  ok "CODEWHALE_CACHE_ROOT overrides XDG and HOME"
else
  bad "CODEWHALE_CACHE_ROOT overrides XDG and HOME" "$out"
fi

# 3. Disabled mode leaves Cargo/sccache vars alone.
out=$(run_apply CODEWHALE_DEV_CACHE=0)
if contains "$out" "mode=disabled" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=<unset>" \
  && contains "$out" "RUSTC_WRAPPER=<unset>"; then
  ok "CODEWHALE_DEV_CACHE=0 does not set cargo or sccache vars"
else
  bad "CODEWHALE_DEV_CACHE=0 does not set cargo or sccache vars" "$out"
fi

# 4. New worktree (no ./target) gets isolated build-dir with the cargo template.
out=$(run_apply \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "mode=isolated-build-dir" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=$work/cache/build/{workspace-path-hash}"; then
  ok "new worktree uses isolated build-dir template"
else
  bad "new worktree uses isolated build-dir template" "$out"
fi
if contains "$out" "CARGO_INCREMENTAL=<unset>"; then
  ok "isolated default does not turn incremental off"
else
  bad "isolated default does not turn incremental off" "$out"
fi
if contains "$out" "sccache=skipped-incremental"; then
  ok "sccache stays off while incremental is on"
else
  bad "sccache stays off while incremental is on" "$out"
fi

# 5. A stub or leftover ./target does not abandon isolation (Cargo still
# writes CACHEDIR.TAG under ./target when build-dir is split).
out=$(run_apply \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$OLD_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "mode=isolated-build-dir" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=$work/cache/build/{workspace-path-hash}"; then
  ok "existing ./target does not abandon isolated build-dir"
else
  bad "existing ./target does not abandon isolated build-dir" "$out"
fi

# 6. CODEWHALE_DEV_CACHE=local keeps ./target.
out=$(run_apply \
  CODEWHALE_DEV_CACHE=local \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$OLD_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "mode=existing-target" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=<unset>"; then
  ok "CODEWHALE_DEV_CACHE=local keeps ./target"
else
  bad "CODEWHALE_DEV_CACHE=local keeps ./target" "$out"
fi

# 7. Already-set CARGO_TARGET_DIR / CARGO_BUILD_BUILD_DIR are respected.
out=$(run_apply \
  CODEWHALE_DEV_CACHE=1 \
  CARGO_TARGET_DIR="$work/user-target" \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT")
if contains "$out" "mode=inherited-target-dir" \
  && contains "$out" "CARGO_TARGET_DIR=$work/user-target" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=<unset>"; then
  ok "existing CARGO_TARGET_DIR is not overwritten"
else
  bad "existing CARGO_TARGET_DIR is not overwritten" "$out"
fi
out=$(run_apply \
  CARGO_BUILD_BUILD_DIR="$work/user-build" \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT")
if contains "$out" "mode=inherited-build-dir" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=$work/user-build"; then
  ok "existing CARGO_BUILD_BUILD_DIR is not overwritten"
else
  bad "existing CARGO_BUILD_BUILD_DIR is not overwritten" "$out"
fi

# 8. Missing sccache is a fallback, not a failure, even when requested.
out=$(run_apply \
  CODEWHALE_SCCACHE=1 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "sccache=not-found" \
  && contains "$out" "RUSTC_WRAPPER=<unset>" \
  && contains "$out" "CARGO_INCREMENTAL=0"; then
  ok "missing sccache falls back without setting RUSTC_WRAPPER"
else
  bad "missing sccache falls back without setting RUSTC_WRAPPER" "$out"
fi

# 9. sccache wraps only when incremental is off and the binary exists.
printf '%s\n' '#!/bin/sh' 'exit 0' >"$FAKEBIN/sccache"
chmod +x "$FAKEBIN/sccache"

out=$(run_apply \
  CARGO_INCREMENTAL=0 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache" \
  CODEWHALE_RUSTC_COMMIT=abc123deadbeef)
if contains "$out" "sccache=enabled" \
  && contains "$out" "RUSTC_WRAPPER=$FAKEBIN/sccache" \
  && contains "$out" "SCCACHE_DIR=$work/cache/sccache/abc123deadbeef"; then
  ok "sccache wraps rustc when incremental is off"
else
  bad "sccache wraps rustc when incremental is off" "$out"
fi

out=$(run_apply \
  CARGO_INCREMENTAL=1 \
  CODEWHALE_SCCACHE=1 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "sccache=skipped-incremental" \
  && contains "$out" "RUSTC_WRAPPER=<unset>" \
  && contains "$out" "CARGO_INCREMENTAL=1"; then
  ok "explicit incremental=1 wins over CODEWHALE_SCCACHE=1"
else
  bad "explicit incremental=1 wins over CODEWHALE_SCCACHE=1" "$out"
fi

out=$(run_apply \
  CODEWHALE_SCCACHE=1 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache" \
  CODEWHALE_RUSTC_COMMIT=abc123deadbeef)
if contains "$out" "sccache=enabled" \
  && contains "$out" "CARGO_INCREMENTAL=0"; then
  ok "CODEWHALE_SCCACHE=1 requests incremental=0 and wraps"
else
  bad "CODEWHALE_SCCACHE=1 requests incremental=0 and wraps" "$out"
fi

out=$(run_apply \
  RUSTC_WRAPPER=/usr/bin/true \
  CARGO_INCREMENTAL=0 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT")
if contains "$out" "sccache=external-wrapper" \
  && contains "$out" "RUSTC_WRAPPER=/usr/bin/true"; then
  ok "existing RUSTC_WRAPPER is left alone"
else
  bad "existing RUSTC_WRAPPER is left alone" "$out"
fi

# 10. Cargo older than 1.91 falls back to a per-workspace CARGO_TARGET_DIR.
printf '%s\n' '#!/bin/sh' 'echo "cargo 1.88.0 (test)"' >"$FAKEBIN/cargo"
chmod +x "$FAKEBIN/cargo"
out=$(run_apply \
  CODEWHALE_DEV_CACHE=1 \
  CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
  CODEWHALE_CACHE_ROOT="$work/cache")
if contains "$out" "legacy-target-dir" \
  && contains "$out" "CARGO_BUILD_BUILD_DIR=<unset>" \
  && contains "$out" "CARGO_TARGET_DIR=$work/cache/target/"; then
  ok "cargo 1.88 falls back to an isolated CARGO_TARGET_DIR"
else
  bad "cargo 1.88 falls back to an isolated CARGO_TARGET_DIR" "$out"
fi
rm -f "$FAKEBIN/cargo"

# 11. CLI self-check passes for a new worktree with portable HOME.
cli_out=$(
  env -i \
    PATH="$BASE_PATH" \
    HOME="$HOME_DIR" \
    PWD="$NEW_WT" \
    CODEWHALE_DEV_CACHE_QUIET=1 \
    CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
    CODEWHALE_CACHE_ROOT="$work/cache" \
    /bin/sh "$DEV_CACHE" --self-check
) || cli_rc=$?
cli_rc=${cli_rc:-0}
if [ "$cli_rc" -eq 0 ] && contains "$cli_out" "dev-cache: self-check ok"; then
  ok "CLI --self-check passes on a new worktree"
else
  bad "CLI --self-check passes on a new worktree" "$cli_out"
fi

# 12. Two worktree paths produce different legacy fingerprints.
# shellcheck source=scripts/dev-cache.sh
. "$DEV_CACHE"
fp_a=$(codewhale_dev_cache_path_fingerprint "$NEW_WT")
fp_b=$(codewhale_dev_cache_path_fingerprint "$OLD_WT")
if [ -n "$fp_a" ] && [ -n "$fp_b" ] && [ "$fp_a" != "$fp_b" ]; then
  ok "path fingerprints differ across worktrees"
else
  bad "path fingerprints differ across worktrees" "a=$fp_a b=$fp_b"
fi

# 13. dev-cargo.sh --status is the same helper.
if /bin/sh "$DEV_CARGO" --help | grep -q 'dev-cache'; then
  ok "dev-cargo.sh --help delegates to the cache helper"
else
  bad "dev-cargo.sh --help delegates to the cache helper"
fi

if [ "$fail" -eq 0 ]; then
  printf 'dev-cache.test.sh: all %s checks passed\n' "$pass"
  exit 0
fi
printf 'dev-cache.test.sh: %s/%s checks failed\n' "$fail" "$((pass + fail))" >&2
exit 1
