#!/bin/sh
# Hermetic tests for scripts/dev-test.sh: area routing, nextest vs
# libtest selection, and that the cache helper is actually applied.
# Uses a fake cargo; does not compile anything.
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
DEV_TEST=$repo_root/scripts/dev-test.sh

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

work=$(mktemp -d "${TMPDIR:-/tmp}/codewhale-dev-test.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup EXIT INT HUP TERM

FAKEBIN=$work/bin
mkdir -p "$FAKEBIN"
NEW_WT=$work/new-wt
mkdir -p "$NEW_WT"

cat >"$FAKEBIN/cargo" <<'EOF'
#!/bin/sh
printf 'CARGO:%s\n' "$*"
printf 'CARGO_BUILD_BUILD_DIR=%s\n' "${CARGO_BUILD_BUILD_DIR-<unset>}"
printf 'RUSTC_WRAPPER=%s\n' "${RUSTC_WRAPPER-<unset>}"
printf 'CARGO_INCREMENTAL=%s\n' "${CARGO_INCREMENTAL-<unset>}"
printf 'RUST_MIN_STACK=%s\n' "${RUST_MIN_STACK-<unset>}"
exit 0
EOF
chmod +x "$FAKEBIN/cargo"

BASE_PATH=$FAKEBIN:/usr/bin:/bin

run_dev_test() {
  # Leading KEY=VAL pairs are extra env; the rest are script arguments.
  _extra=$work/extra-env
  : >"$_extra"
  while [ $# -gt 0 ]; do
    case $1 in
      *=*)
        printf '%s\n' "$1" >>"$_extra"
        shift
        ;;
      *)
        break
        ;;
    esac
  done
  # env -i + env-file keeps values with spaces intact and never treats
  # --list as an env(1) option.
  set -- /bin/sh "$DEV_TEST" "$@"
  while IFS= read -r _line; do
    set -- "$_line" "$@"
  done <"$_extra"
  env -i \
    PATH="$BASE_PATH" \
    HOME="$work/home" \
    CODEWHALE_DEV_CACHE_QUIET=1 \
    CODEWHALE_DEV_CACHE_REPO_ROOT="$NEW_WT" \
    CODEWHALE_CACHE_ROOT="$work/cache" \
    "$@"
}

contains() {
  printf '%s' "$1" | grep -qF -- "$2"
}

# --list names every workspace crate family, including the ones the old
# mapper dropped on the floor.
list_out=$(run_dev_test --list)
for needle in \
  "codewhale-agent" \
  "codewhale-app-server" \
  "codewhale-build-support" \
  "codewhale-command-contract" \
  "codewhale-hooks" \
  "codewhale-lane" \
  "codewhale-mcp" \
  "codewhale-release" \
  "codewhale-state" \
  "codewhale-telemetry" \
  "codewhale-workflow" \
  "codewhale-workflow-js" \
  "codewhale-tui --lib" \
  "codewhale-tui --test pty"
do
  if contains "$list_out" "$needle"; then
    ok "--list mentions $needle"
  else
    bad "--list mentions $needle" "$list_out"
  fi
done

# Narrow routing: libtest forced so we assert cargo test argv.
out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 config)
if contains "$out" "+ cargo test -p codewhale-config --lib --locked" \
  && contains "$out" "test -p codewhale-config --lib --locked"; then
  ok "area config maps to codewhale-config --lib"
else
  bad "area config maps to codewhale-config --lib" "$out"
fi

out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 crates/tui/src/elapsed.rs)
if contains "$out" "test -p codewhale-tui --lib --locked elapsed::"; then
  ok "path crates/tui/src/elapsed.rs invents elapsed::"
else
  bad "path crates/tui/src/elapsed.rs invents elapsed::" "$out"
fi

out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 crates/workflow-js/src/eval.rs)
if contains "$out" "test -p codewhale-workflow-js --lib --locked"; then
  ok "path crates/workflow-js maps to codewhale-workflow-js"
else
  bad "path crates/workflow-js maps to codewhale-workflow-js" "$out"
fi

out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 crates/app-server/src/lib.rs)
if contains "$out" "test -p codewhale-app-server --lib --locked"; then
  ok "path crates/app-server maps to codewhale-app-server"
else
  bad "path crates/app-server maps to codewhale-app-server" "$out"
fi

out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 tui-pty qa_pty)
if contains "$out" "test -p codewhale-tui --test pty --locked qa_pty"; then
  ok "tui-pty maps to the pty harness"
else
  bad "tui-pty maps to the pty harness" "$out"
fi

# Unknown area fails closed.
rc=0
out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 not-a-crate 2>&1) || rc=$?
if [ "$rc" -ne 0 ] && contains "$out" "unknown area"; then
  ok "unknown area fails closed"
else
  bad "unknown area fails closed" "rc=$rc $out"
fi

# New worktree applies the isolated build-dir (compile-time topology).
out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 config)
if contains "$out" "CARGO_BUILD_BUILD_DIR=$work/cache/build/{workspace-path-hash}" \
  && contains "$out" '--config build.build-dir = "'; then
  ok "dev-test applies isolated build-dir on a new worktree"
else
  bad "dev-test applies isolated build-dir on a new worktree" "$out"
fi
if contains "$out" "RUST_MIN_STACK=16777216"; then
  ok "dev-test sets RUST_MIN_STACK when unset"
else
  bad "dev-test sets RUST_MIN_STACK when unset" "$out"
fi
if contains "$out" "RUSTC_WRAPPER=<unset>"; then
  ok "dev-test does not wrap rustc on the incremental default"
else
  bad "dev-test does not wrap rustc on the incremental default" "$out"
fi

# CODEWHALE_DEV_CACHE=0 really disables the topology.
out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 CODEWHALE_DEV_CACHE=0 config)
if contains "$out" "CARGO_BUILD_BUILD_DIR=<unset>" \
  && ! contains "$out" '--config build.build-dir'; then
  ok "CODEWHALE_DEV_CACHE=0 skips the cache topology"
else
  bad "CODEWHALE_DEV_CACHE=0 skips the cache topology" "$out"
fi

# nextest is used when cargo-nextest is on PATH (test-runtime).
printf '%s\n' '#!/bin/sh' 'exit 0' >"$FAKEBIN/cargo-nextest"
chmod +x "$FAKEBIN/cargo-nextest"
out=$(run_dev_test tui elapsed::)
if contains "$out" "+ cargo nextest run -p codewhale-tui --lib --locked elapsed::" \
  && contains "$out" "nextest run -p codewhale-tui --lib --locked elapsed::"; then
  ok "nextest is used when cargo-nextest is installed"
else
  bad "nextest is used when cargo-nextest is installed" "$out"
fi

out=$(run_dev_test CODEWHALE_DEV_NEXTEST=0 tui elapsed::)
if contains "$out" "+ cargo test -p codewhale-tui --lib --locked elapsed::" \
  && contains "$out" "test -p codewhale-tui --lib --locked elapsed::"; then
  ok "CODEWHALE_DEV_NEXTEST=0 forces cargo test"
else
  bad "CODEWHALE_DEV_NEXTEST=0 forces cargo test" "$out"
fi

# Forced nextest without the binary fails closed instead of silent cargo.
rm -f "$FAKEBIN/cargo-nextest"
rc=0
out=$(run_dev_test CODEWHALE_DEV_NEXTEST=1 config 2>&1) || rc=$?
if [ "$rc" -ne 0 ] && contains "$out" "cargo-nextest is not on PATH"; then
  ok "CODEWHALE_DEV_NEXTEST=1 without nextest fails closed"
else
  bad "CODEWHALE_DEV_NEXTEST=1 without nextest fails closed" "rc=$rc $out"
fi

if [ "$fail" -eq 0 ]; then
  printf 'dev-test.test.sh: all %s checks passed\n' "$pass"
  exit 0
fi
printf 'dev-test.test.sh: %s/%s checks failed\n' "$fail" "$((pass + fail))" >&2
exit 1
