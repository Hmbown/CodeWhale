#!/usr/bin/env bash
# Mobile runtime surface smoke tests.
# Launches the compiled codewhale-tui binary on loopback ports and verifies
# the mobile control page, auth, API routes, and binding behaviour through
# real HTTP requests.
#
# Usage:  ./scripts/mobile-smoke.sh
# Requires: curl, a built binary at target/release/codewhale-tui
#           (the script will build it if cargo is available).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="${BINARY:-${REPO_ROOT}/target/release/codewhale-tui}"
PASS=0
FAIL=0
SERVER_PID=""

# ── helpers ──────────────────────────────────────────────────────────────────

log()  { printf "\033[1;34m>>> %s\033[0m\n" "$*"; }
pass() { printf "\033[1;32m  ✓ %s\033[0m\n" "$*"; PASS=$((PASS + 1)); }
fail() { printf "\033[1;31m  ✗ %s\033[0m\n" "$*"; FAIL=$((FAIL + 1)); }

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

pick_port() {
    # Find a free TCP port on loopback.
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

start_server() {
    local port="$1"; shift
    log "Starting server on port $port: $*"
    "$BINARY" serve --port "$port" "$@" &
    SERVER_PID=$!
    # Wait for the server to become ready.
    for _ in $(seq 1 30); do
        if curl -sf "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.3
    done
    fail "Server did not become ready on port $port"
    cleanup
    return 1
}

stop_server() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        SERVER_PID=""
    fi
}

# assert_status METHOD PATH [HEADER_NAME:HEADER_VALUE] EXPECTED_STATUS
assert_status() {
    local method="$1" path="$2" header="" expected=""
    if [[ $# -eq 5 ]]; then
        header="$3"; expected="$4"
    else
        expected="$3"
    fi

    local url="http://127.0.0.1:${PORT}${path}"
    local curl_args=(-sf -o /dev/null -w '%{http_code}' -X "$method")
    if [[ -n "$header" ]]; then
        curl_args+=(-H "$header")
    fi

    local actual
    actual=$(curl "${curl_args[@]}" "$url" 2>/dev/null || true)

    if [[ "$actual" == "$expected" ]]; then
        pass "$method $path → $expected"
    else
        fail "$method $path → expected $expected, got $actual"
    fi
}

# assert_body_contains METHOD PATH HEADER BODY_SUBSTRING
assert_body_contains() {
    local method="$1" path="$2" header="$3" substring="$4"
    local url="http://127.0.0.1:${PORT}${path}"
    local curl_args=(-sf -X "$method")
    if [[ -n "$header" ]]; then
        curl_args+=(-H "$header")
    fi

    local body
    body=$(curl "${curl_args[@]}" "$url" 2>/dev/null || true)

    if echo "$body" | grep -q "$substring"; then
        pass "$method $path body contains '$substring'"
    else
        fail "$method $path body missing '$substring'"
    fi
}

# ── build ────────────────────────────────────────────────────────────────────

if [[ ! -x "$BINARY" ]]; then
    log "Binary not found; building codewhale-tui in release mode..."
    cargo build -p codewhale-tui --release --locked
fi

log "Using binary: $BINARY"

# ── Test Group 1: Token auth ────────────────────────────────────────────────

TOKEN="smoke_test_token_$$"
PORT=$(pick_port)

log "=== Test Group 1: Token auth ==="
start_server "$PORT" --mobile --auth-token "$TOKEN"

assert_status GET "/mobile" 401
assert_body_contains GET "/mobile?token=${TOKEN}" "" "CodeWhale Mobile"
assert_status GET "/v1/threads/summary" 401
assert_status GET "/v1/threads/summary" "Authorization: Bearer ${TOKEN}" 200
assert_status POST "/v1/approvals/no_such_id" "Authorization: Bearer ${TOKEN}" 404

stop_server

# ── Test Group 2: Insecure mode ─────────────────────────────────────────────

PORT=$(pick_port)

log "=== Test Group 2: Insecure mode (no token) ==="
start_server "$PORT" --mobile --insecure

assert_body_contains GET "/mobile" "" "CodeWhale Mobile"
assert_status GET "/v1/threads/summary" 200

stop_server

# ── Test Group 3: Binding warnings ──────────────────────────────────────────

PORT=$(pick_port)

log "=== Test Group 3: Binding warnings (0.0.0.0 default) ==="
STDOUT_FILE=$(mktemp)
"$BINARY" serve --port "$PORT" --mobile --insecure > "$STDOUT_FILE" 2>&1 &
SERVER_PID=$!
sleep 2
STDOUT=$(cat "$STDOUT_FILE")
rm -f "$STDOUT_FILE"

if echo "$STDOUT" | grep -q "0.0.0.0"; then
    pass "stdout/stderr contains 0.0.0.0 binding warning"
else
    fail "stdout/stderr missing 0.0.0.0 binding warning"
fi

if echo "$STDOUT" | grep -qi "mobile"; then
    pass "stdout contains mobile URL hint"
else
    fail "stdout missing mobile URL hint"
fi

stop_server

# ── summary ──────────────────────────────────────────────────────────────────

echo ""
log "Results: $PASS passed, $FAIL failed"

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
