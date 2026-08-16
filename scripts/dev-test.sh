#!/bin/sh
# Map a workspace area or source path to the fastest cargo/nextest
# invocation for that area, and apply the portable cache topology so a
# new worktree actually gets isolated build-dir (+ sccache only when
# incremental is already off). Developer iteration aid only; no product
# behavior.
#
# Usage:
#   scripts/dev-test.sh <area|path> [filter...]
#   scripts/dev-test.sh --list
#   scripts/dev-test.sh --status
#
# Examples:
#   scripts/dev-test.sh config
#   scripts/dev-test.sh tui elapsed::
#   scripts/dev-test.sh crates/tui/src/elapsed.rs
#   scripts/dev-test.sh tui-pty qa_pty
#
# Environment:
#   CODEWHALE_DEV_NEXTEST  auto|1|0  (default auto: use cargo-nextest when
#                          it is on PATH; 0 forces cargo test)
#   Cache knobs are documented in scripts/dev-cache.sh.
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# shellcheck source=scripts/dev-cache.sh
. "$repo_root/scripts/dev-cache.sh"

usage() {
  printf '%s\n' "usage: scripts/dev-test.sh <area|path> [filter...]" >&2
  printf '%s\n' "       scripts/dev-test.sh --list|--status|--self-check" >&2
  exit 2
}

list_areas() {
  cat <<'EOF'
area              command
----              -------
agent             cargo test -p codewhale-agent --lib --locked
app-server        cargo test -p codewhale-app-server --lib --locked
build-support     cargo test -p codewhale-build-support --lib --locked
cli               cargo test -p codewhale-cli --lib --locked
command-contract  cargo test -p codewhale-command-contract --lib --locked
config            cargo test -p codewhale-config --lib --locked
core              cargo test -p codewhale-core --lib --locked
execpolicy        cargo test -p codewhale-execpolicy --lib --locked
hooks             cargo test -p codewhale-hooks --lib --locked
lane              cargo test -p codewhale-lane --lib --locked
mcp               cargo test -p codewhale-mcp --lib --locked
paths             cargo test -p codewhale-paths --lib --locked
protocol          cargo test -p codewhale-protocol --lib --locked
release           cargo test -p codewhale-release --lib --locked
secrets           cargo test -p codewhale-secrets --lib --locked
state             cargo test -p codewhale-state --lib --locked
telemetry         cargo test -p codewhale-telemetry --lib --locked
tools             cargo test -p codewhale-tools --lib --locked
tui               cargo test -p codewhale-tui --lib --locked
tui-integration   cargo test -p codewhale-tui --test integration --locked
tui-pty           cargo test -p codewhale-tui --test pty --locked
tui-cucumber      cargo test -p codewhale-tui --test cucumber --locked
workflow          cargo test -p codewhale-workflow --lib --locked
workflow-js       cargo test -p codewhale-workflow-js --lib --locked

path prefix                         area / extra filter
-----------                         -------------------
crates/<crate>/                     <crate>
crates/tui/src/tui/                 tui  tui::
crates/tui/src/tools/               tui  tools::
crates/tui/src/core/                tui  core::
crates/tui/src/commands/            tui  commands::
crates/tui/src/<file>.rs            tui  <file>::
crates/tui/tests/integration/       tui-integration  <stem>
crates/tui/tests/pty/               tui-pty  <stem>
crates/tui/tests/cucumber/          tui-cucumber  <stem>
crates/tui/tests/                  tui-integration

When cargo-nextest is on PATH, the run stage is `cargo nextest run`
instead of `cargo test` (same binaries; process per test). Set
CODEWHALE_DEV_NEXTEST=0 to force libtest. New worktrees get an isolated
Cargo build-dir via scripts/dev-cache.sh; sccache wraps rustc only when
incremental is already off. Do not use cargo test --workspace for a
single-area edit. --lib and --tests are disjoint; a green --lib run does
not cover crates/tui/tests/.
EOF
}

[ $# -ge 1 ] || usage

if [ "$1" = "--list" ] || [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
  list_areas
  exit 0
fi

if [ "$1" = "--status" ] || [ "$1" = "--self-check" ]; then
  exec "$repo_root/scripts/dev-cache.sh" "$1"
fi

area=$1
shift

# Path form: map a source path onto an area, and invent a filter only when
# the caller did not already pass one.
if [ -e "$area" ] || printf '%s' "$area" | grep -q /; then
  rel=${area#./}
  extra=
  case $rel in
    crates/tui/tests/integration/*)
      area=tui-integration
      extra=$(basename "$rel" .rs)
      ;;
    crates/tui/tests/pty/*)
      area=tui-pty
      extra=$(basename "$rel" .rs)
      ;;
    crates/tui/tests/cucumber/*)
      area=tui-cucumber
      extra=$(basename "$rel" .rs)
      ;;
    crates/tui/tests/*)
      area=tui-integration
      extra=$(basename "$rel" .rs)
      ;;
    crates/tui/src/tui/*)
      area=tui
      extra=tui::
      ;;
    crates/tui/src/tools/*)
      area=tui
      extra=tools::
      ;;
    crates/tui/src/core/*)
      area=tui
      extra=core::
      ;;
    crates/tui/src/commands/*)
      area=tui
      extra=commands::
      ;;
    crates/tui/src/*)
      area=tui
      extra=$(basename "$rel" .rs)::
      ;;
    crates/tui/*|crates/tui)
      area=tui
      ;;
    crates/*)
      crate=$(printf '%s' "$rel" | awk -F/ '{print $2}')
      area=$crate
      ;;
    *)
      printf '%s\n' "dev-test: no area mapping for path: $area" >&2
      exit 2
      ;;
  esac
  case $extra in
    ''|main|main::|lib::|mod|mod::) extra= ;;
  esac
  if [ $# -eq 0 ] && [ -n "${extra:-}" ]; then
    set -- "$extra"
  fi
fi

pkg=
target=--lib
case $area in
  agent) pkg=codewhale-agent ;;
  app-server) pkg=codewhale-app-server ;;
  build-support) pkg=codewhale-build-support ;;
  cli) pkg=codewhale-cli ;;
  command-contract) pkg=codewhale-command-contract ;;
  config) pkg=codewhale-config ;;
  core) pkg=codewhale-core ;;
  execpolicy) pkg=codewhale-execpolicy ;;
  hooks) pkg=codewhale-hooks ;;
  lane) pkg=codewhale-lane ;;
  mcp) pkg=codewhale-mcp ;;
  paths) pkg=codewhale-paths ;;
  protocol) pkg=codewhale-protocol ;;
  release) pkg=codewhale-release ;;
  secrets) pkg=codewhale-secrets ;;
  state) pkg=codewhale-state ;;
  telemetry) pkg=codewhale-telemetry ;;
  tools) pkg=codewhale-tools ;;
  tui) pkg=codewhale-tui ;;
  workflow) pkg=codewhale-workflow ;;
  workflow-js) pkg=codewhale-workflow-js ;;
  tui-integration)
    pkg=codewhale-tui
    target=--test
    harness=integration
    ;;
  tui-pty)
    pkg=codewhale-tui
    target=--test
    harness=pty
    ;;
  tui-cucumber)
    pkg=codewhale-tui
    target=--test
    harness=cucumber
    ;;
  *)
    printf '%s\n' "dev-test: unknown area: $area (try --list)" >&2
    exit 2
    ;;
esac

codewhale_dev_cache_apply
if [ -z "${RUST_MIN_STACK:-}" ]; then
  RUST_MIN_STACK=16777216
  export RUST_MIN_STACK
fi

use_nextest=0
_cw_nextest=${CODEWHALE_DEV_NEXTEST:-auto}
if codewhale_dev_cache_falsey "$_cw_nextest"; then
  use_nextest=0
elif command -v cargo-nextest >/dev/null 2>&1; then
  use_nextest=1
elif codewhale_dev_cache_truthy "$_cw_nextest"; then
  printf '%s\n' "dev-test: CODEWHALE_DEV_NEXTEST=${_cw_nextest} but cargo-nextest is not on PATH" >&2
  exit 2
fi

if [ "$target" = "--test" ]; then
  if [ "$use_nextest" -eq 1 ]; then
    set -- nextest run -p "$pkg" --test "$harness" --locked "$@"
    printf '+ cargo %s\n' "$*"
    codewhale_dev_cache_exec_cargo "$@"
  fi
  set -- test -p "$pkg" --test "$harness" --locked "$@"
else
  if [ "$use_nextest" -eq 1 ]; then
    set -- nextest run -p "$pkg" --lib --locked "$@"
    printf '+ cargo %s\n' "$*"
    codewhale_dev_cache_exec_cargo "$@"
  fi
  set -- test -p "$pkg" --lib --locked "$@"
fi

printf '+ cargo %s\n' "$*"
codewhale_dev_cache_exec_cargo "$@"
