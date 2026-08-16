#!/bin/sh
# Run cargo with the portable Codewhale cache topology applied.
#
#   scripts/dev-cargo.sh test -p codewhale-config --lib --locked
#   scripts/dev-cargo.sh --status
#   scripts/dev-cargo.sh --self-check
#
# This is the everyday compile entry point. It does not change product
# behavior. See scripts/dev-cache.sh and docs/BUILD_PERFORMANCE.md.
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# shellcheck source=scripts/dev-cache.sh
. "$repo_root/scripts/dev-cache.sh"

case ${1:-} in
  --status|--self-check|--print-exports|--help|-h)
    exec "$repo_root/scripts/dev-cache.sh" "$1"
    ;;
esac

codewhale_dev_cache_apply
# Match CI / the product's 16 MiB owner-thread stack so local cargo test
# and nextest do not abort on the default ~2 MiB (Windows ~1 MiB) stack.
if [ -z "${RUST_MIN_STACK:-}" ]; then
  RUST_MIN_STACK=16777216
  export RUST_MIN_STACK
fi
codewhale_dev_cache_exec_cargo "$@"
