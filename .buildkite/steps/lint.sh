#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=/dev/null
. .buildkite/steps/common.sh

rustup component add rustfmt clippy

echo "--- cargo fmt"
cargo fmt --all -- --check

# The allow list is copied verbatim from .github/workflows/ci.yml. Keep the two
# in step: a lint that is denied there and allowed here makes this pipeline a
# weaker gate that still reports green.
echo "--- cargo clippy"
cargo clippy --workspace --all-targets --all-features --locked -- \
  -D warnings \
  -A clippy::uninlined_format_args \
  -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or
