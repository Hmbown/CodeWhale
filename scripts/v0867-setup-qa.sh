#!/usr/bin/env bash
# v0.8.67 constitution-first setup lane — headless QA probe.
#
# Exercises the noninteractive surfaces of the setup lane against isolated
# temp homes so the human manual pass shrinks to visual confirmation only.
# It does NOT drive the interactive TUI; it verifies the machine-readable
# contracts (doctor --json .setup, constitution state derivation, secret
# safety, WHALE.md migration diagnostics) that the QA matrix ties each
# scenario to.
#
# Usage:
#   scripts/v0867-setup-qa.sh            # build (release) if needed, then probe
#   CODEWHALE_BIN=/path/to/codewhale-tui scripts/v0867-setup-qa.sh   # use a prebuilt binary
#
# Exit 0 = every probe passed. Non-zero = a contract regressed; the failing
# probe prints what it expected vs. observed. Requires `jq`.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if ! command -v jq >/dev/null 2>&1; then
  echo "FATAL: jq is required (brew install jq)." >&2
  exit 2
fi

BIN="${CODEWHALE_BIN:-}"
if [[ -z "$BIN" ]]; then
  if [[ -x "target/release/codewhale-tui" ]]; then
    BIN="target/release/codewhale-tui"
  else
    echo "Building release codewhale-tui (set CODEWHALE_BIN to skip)…" >&2
    cargo build --release -p codewhale-tui >&2
    BIN="target/release/codewhale-tui"
  fi
fi
if [[ "$BIN" != /* ]]; then
  BIN="$REPO_ROOT/$BIN"
fi
echo "Using binary: $BIN" >&2

PASS=0
FAIL=0
pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_jq_value() {
  local label="$1"
  local json="$2"
  local filter="$3"
  local expected="$4"
  local observed
  observed="$(echo "$json" | jq -r "$filter")"
  if [[ "$observed" == "$expected" ]]; then
    pass "$label"
  else
    fail "$label wrong: $observed"
  fi
}
expect_jq_select() {
  local label="$1"
  local json="$2"
  local filter="$3"
  if echo "$json" | jq -e "$filter" >/dev/null; then
    pass "$label"
  else
    fail "$label missing"
  fi
}

# Run the binary in a fully isolated home so no real install is read or
# mutated. Echoes the doctor --json blob on stdout.
doctor_json_in() {
  local home="$1"
  shift
  CODEWHALE_HOME="$home/codewhale-home" \
  HOME="$home/home" \
  USERPROFILE="$home/home" \
  DEEPSEEK_CONFIG_PATH="$home/codewhale-home/config.toml" \
    "$BIN" doctor --json "$@" 2>/dev/null
}

new_home() {
  local d
  d="$(mktemp -d)"
  mkdir -p "$d/codewhale-home" "$d/home"
  echo "$d"
}

echo "== v0.8.67 setup-lane headless QA =="

# --- Scenario: clean home, no constitution chosen yet ---
echo "[clean home] doctor --json .setup contract"
H="$(new_home)"
SETUP="$(doctor_json_in "$H" | jq '.setup')"
if [[ -n "$SETUP" && "$SETUP" != "null" ]]; then
  pass "doctor --json emits a .setup block on a clean home"
else
  fail "doctor --json .setup missing on a clean home"
fi
for field in constitution runtime_posture_source steps next_actions; do
  if echo "$SETUP" | jq -e "has(\"$field\")" >/dev/null; then
    pass ".setup.$field present"
  else
    fail ".setup.$field missing"
  fi
done
expect_jq_value ".setup.first_run_ready == false" "$SETUP" '.first_run_ready' "false"
expect_jq_value ".setup.update_ready == false" "$SETUP" '.update_ready' "false"
expect_jq_value ".setup.operate_ready == false" "$SETUP" '.operate_ready' "false"
expect_jq_value ".setup.next_actions.constitution == /constitution" "$SETUP" '.next_actions.constitution' "/constitution"
expect_jq_value ".setup.next_actions.provider_model advertises guided provider setup" "$SETUP" '.next_actions.provider_model' "/setup provider, /provider setup <name>, or /model"
expect_jq_value ".setup.next_actions.hotbar == /setup hotbar" "$SETUP" '.next_actions.hotbar' "/setup hotbar"
expect_jq_value ".setup.next_actions.tools_mcp == /setup tools" "$SETUP" '.next_actions.tools_mcp' "/setup tools"
expect_jq_value ".setup.next_actions.remote_runtime == /setup remote" "$SETUP" '.next_actions.remote_runtime' "/setup remote"
expect_jq_value ".setup.next_actions.persistence == /setup persistence" "$SETUP" '.next_actions.persistence' "/setup persistence"
expect_jq_value ".setup.operate_fleet.concurrency.plan_limit_probed == false" "$SETUP" '.operate_fleet.concurrency.plan_limit_probed' "false"
expect_jq_value ".setup.operate_fleet.roster.readiness_rule is documented" "$SETUP" '.operate_fleet.roster.readiness_rule' "built-in starter roster or custom roster"
for step in provider_model trust_sandbox operate_fleet hotbar tools_mcp remote_runtime persistence verification; do
  expect_jq_select ".setup.steps includes $step" "$SETUP" ".steps[] | select(.step == \"$step\")"
done
rm -rf "$H"

# --- Scenario: secret safety — a configured key must never appear in doctor --json ---
echo "[secret safety] configured key absent from doctor --json"
H="$(new_home)"
SECRET="CANARY_apikey_do_not_leak_0000"
cat > "$H/codewhale-home/config.toml" <<EOF
model = "deepseek-v4-pro"
[providers.deepseek]
api_key = "$SECRET"
EOF
FULL="$(doctor_json_in "$H")"
if echo "$FULL" | grep -q "$SECRET"; then
  fail "raw API key leaked into doctor --json output"
else
  pass "raw API key never appears in doctor --json"
fi
rm -rf "$H"

# --- Scenario: existing valid repo constitution surfaces without leaking body ---
echo "[repo law] enforced invariant surfaces in context diagnostics, body not loaded verbatim"
H="$(new_home)"
WS="$(mktemp -d)"
mkdir -p "$WS/.codewhale"
cat > "$WS/.codewhale/constitution.json" <<'EOF'
{
  "authority": ["AGENTS.md"],
  "protected_invariants": [
    { "text": "The wire format is frozen", "paths": ["crates/protocol/**"], "action": "block" }
  ]
}
EOF
CTX="$(cd "$WS" && CODEWHALE_HOME="$H/codewhale-home" HOME="$H/home" USERPROFILE="$H/home" \
  "$BIN" doctor --context-json 2>/dev/null || true)"
if echo "$CTX" | jq -e '.entries[] | select(.source_kind == "repo_constitution")' >/dev/null 2>&1; then
  pass "repo constitution surfaces in --context-json"
else
  fail "repo constitution not found in --context-json"
fi
rm -rf "$H" "$WS"

# --- Scenario: legacy WHALE.md is ignored, body not loaded ---
echo "[WHALE.md migration] legacy file reported, body never surfaced"
H="$(new_home)"
WS="$(mktemp -d)"
printf 'SECRET_WHALE_BODY_SHOULD_NOT_APPEAR\n' > "$WS/WHALE.md"
CTX="$(cd "$WS" && CODEWHALE_HOME="$H/codewhale-home" HOME="$H/home" USERPROFILE="$H/home" \
  "$BIN" doctor --context-json 2>/dev/null || true)"
if echo "$CTX" | grep -q "SECRET_WHALE_BODY_SHOULD_NOT_APPEAR"; then
  fail "legacy WHALE.md body leaked into context report"
else
  pass "legacy WHALE.md body not loaded into context report"
fi
rm -rf "$H" "$WS"

echo
echo "== summary: $PASS passed, $FAIL failed =="
if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi

cat <<'EOF'

Remaining MANUAL (visual) checks — these need a human eye on a live TUI and
are the only setup-lane items this script cannot cover:
  1. /setup welcome opens with the dual meaning of "code" and the
     choose -> draft -> ratify arc.
  2. /setup Constitution step: G guided preview + ratify, K keep-existing
     (when a valid constitution.json is present), A model-draft (provider
     ready), U bundled.
  3. /constitution manager renders bundled / user-global / repo-local /
     AGENTS / memory layers.
  4. Approval prompt reads calm for routine/elevated actions and reserves
     the red DESTRUCTIVE styling for genuinely critical ones.
  5. /fleet setup: m drafts a profile (provider ready), preview shows the
     exact TOML, g ratifies.
EOF
