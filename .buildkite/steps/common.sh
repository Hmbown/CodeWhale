#!/usr/bin/env bash
# Shared setup for Buildkite steps. Sourced, not executed.
set -euo pipefail

# Hosted Linux agents are bare containers; macOS agents ship Xcode and brew.
# Only install what the workspace actually links against (dbus, via the
# keyring/secret-store path) so a step failure is about the code, not apt.
if [ "$(uname -s)" = "Linux" ]; then
  SUDO=""
  command -v sudo >/dev/null 2>&1 && SUDO="sudo"
  for i in 1 2 3 4 5; do
    $SUDO apt-get update && break
    echo "apt-get update failed (attempt $i); retrying in 15s" >&2
    sleep 15
  done
  $SUDO apt-get install -y --no-install-recommends \
    ca-certificates curl pkg-config libdbus-1-dev build-essential
fi

# The Linux job runs as root, and test.sh re-execs the suite as an
# unprivileged user (root ignores permission bits, which silently defeats every
# read-only assertion). A toolchain under /root is unreadable after that swap --
# build 1445 got `cargo: command not found` at uid 1000 -- so install it
# somewhere both users can reach.
if [ "$(id -u)" = "0" ] && [ "$(uname -s)" = "Linux" ]; then
  export CARGO_HOME=/opt/cargo
  export RUSTUP_HOME=/opt/rustup
else
  export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
fi
export PATH="$CARGO_HOME/bin:$PATH"

# rust-toolchain.toml pins `stable`; rustup honours it on first cargo call.
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
fi
# shellcheck disable=SC1091
[ -f "$CARGO_HOME/env" ] && . "$CARGO_HOME/env"
export PATH="$CARGO_HOME/bin:$PATH"

# Readable+traversable by the unprivileged user; cargo still needs to write
# into CARGO_HOME for `cargo install`, so that stays root-owned until test.sh
# hands it over.
if [ "$(id -u)" = "0" ] && [ "$(uname -s)" = "Linux" ]; then
  chmod -R a+rX "$CARGO_HOME" "$RUSTUP_HOME" 2>/dev/null || true
fi

cargo --version
rustc --version
