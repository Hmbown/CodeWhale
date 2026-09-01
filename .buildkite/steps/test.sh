#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=/dev/null
. .buildkite/steps/common.sh

# nextest profile `ci` lives in .config/nextest.toml alongside the test-group
# bounds that serialize the binary-spawning integration suites.
if ! command -v cargo-nextest >/dev/null 2>&1; then
  cargo install cargo-nextest --locked --version 0.9.* || cargo install cargo-nextest --locked
fi

# Hosted Linux agents run the job as root. That is not equivalent to GitHub's
# `runner` user: root ignores permission bits, so every test that makes a path
# read-only and asserts the write is refused instead *succeeds* at writing and
# fails the assertion. Build 1443 failed exactly four tests this way --
# an_unwritable_home_reports_the_failure_and_still_answers,
# contract_edit_rejects_read_only_target_before_atomic_replace,
# failed_apply_rolls_back_to_the_prior_document, and one fleet executor case --
# none of which are product defects.
#
# Drop to an unprivileged user rather than skipping them: those tests guard
# data-loss and permission behaviour, and a CI lane that silently cannot
# exercise them is a weaker gate reporting green.
run_suite() {
  echo "--- workspace tests"
  cargo nextest run --workspace --all-features --locked --profile ci
  echo "--- doctests"
  cargo test --workspace --all-features --locked --doc
}

if [ "$(id -u)" = "0" ] && [ "$(uname -s)" = "Linux" ]; then
  id -u builder >/dev/null 2>&1 || useradd -m -s /bin/bash builder
  # cargo writes into CARGO_HOME (registry, git checkouts) and ./target, so
  # both must belong to the user that will actually run the suite.
  chown -R builder:builder . "$CARGO_HOME" "$RUSTUP_HOME" 2>/dev/null || true
  echo "--- re-exec as unprivileged user (root ignores permission bits)"
  exec runuser -u builder -- env \
    HOME=/home/builder \
    PATH="$PATH" CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}" \
    CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}" \
    RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}" \
    bash -eo pipefail -c '
      cd "$1"
      echo "--- workspace tests (uid $(id -u))"
      cargo nextest run --workspace --all-features --locked --profile ci
      echo "--- doctests"
      cargo test --workspace --all-features --locked --doc
    ' _ "$PWD"
fi

run_suite
