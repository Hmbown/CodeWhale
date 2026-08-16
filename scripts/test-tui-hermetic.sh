#!/bin/sh
# Run the TUI bin tests without reading or writing the developer's Codewhale
# settings/auth home. Rust toolchains remain real through explicit homes.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
real_cargo_home=${CARGO_HOME:-${HOME}/.cargo}
real_rustup_home=${RUSTUP_HOME:-${HOME}/.rustup}
rustc_bin=$(RUSTUP_HOME="$real_rustup_home" rustup which rustc)
toolchain_bin=${rustc_bin%/*}
runs=${TUI_HERMETIC_RUNS:-2}
filter=${TUI_HERMETIC_FILTER:-}

"$repo_root/scripts/check-tui-product-vocabulary.sh"

cleanup_roots=""
cleanup() {
  for root in $cleanup_roots; do
    rm -rf -- "$root"
  done
}
trap cleanup EXIT HUP INT TERM

run=1
while [ "$run" -le "$runs" ]; do
  root=$(mktemp -d "${TMPDIR:-/tmp}/codewhale-tui-test.XXXXXX")
  cleanup_roots="$cleanup_roots $root"
  mkdir -p "$root/home" "$root/codewhale-home" "$root/xdg"
  mkdir -p "$root/codex" "$root/grok" "$root/kimi-code" "$root/kimi-share" "$root/claude"
  printf '%s\n' "hermetic TUI run $run/$runs: $root"
  (
    cd "$repo_root"
    HOME="$root/home" \
    USERPROFILE="$root/home" \
    CODEWHALE_HOME="$root/codewhale-home" \
    XDG_CONFIG_HOME="$root/xdg" \
    CODEWHALE_CONFIG_PATH="$root/codewhale-home/config.toml" \
    CODEX_HOME="$root/codex" \
    GROK_HOME="$root/grok" \
    GROK_AUTH_PATH="$root/grok/auth.json" \
    KIMI_CODE_HOME="$root/kimi-code" \
    KIMI_SHARE_DIR="$root/kimi-share" \
    CLAUDE_CONFIG_DIR="$root/claude" \
    DEEPSEEK_API_KEY= \
    OPENAI_API_KEY= \
    ANTHROPIC_API_KEY= \
    XAI_API_KEY= \
    GROK_API_KEY= \
    MOONSHOT_API_KEY= \
    KIMI_API_KEY= \
    XIAOMI_MIMO_API_KEY= \
    XIAOMI_MIMO_TOKEN_PLAN_API_KEY= \
    MIMO_API_KEY= \
    MIMO_TOKEN_PLAN_API_KEY= \
    CARGO_HOME="$real_cargo_home" \
    RUSTUP_HOME="$real_rustup_home" \
    PATH="$toolchain_bin:$PATH" \
      sh -c '
        if [ -n "$1" ]; then
          exec "$2" test --quiet -p codewhale-tui --bin codewhale-tui --locked "$1"
        fi
        exec "$2" test --quiet -p codewhale-tui --bin codewhale-tui --locked
      ' sh "$filter" "$toolchain_bin/cargo"
  )
  run=$((run + 1))
done
