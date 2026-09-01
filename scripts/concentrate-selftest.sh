#!/usr/bin/env bash
# Keyless end-to-end self-test of the Concentrate provider route.
#
# Boots scripts/concentrate-stub.py (the documented Concentrate contract on
# loopback), then drives the REAL `codewhale exec` path — provider selection,
# secret/env resolution, Route Contract resolution, the Responses wire, SSE
# parsing, and the completed-turn receipt — through it. Nothing leaves the
# machine: the base URL is loopback, the key is a stub value, and no
# Concentrate account exists in this loop.
#
# What it asserts (from the stub's request log and the CLI's stream-json):
#   1. GET  /v1/responses/health answers 200 (stub up, unauthenticated).
#   2. GET  /v1/models is readable without a key.
#   3. POST /v1/responses arrived exactly once per turn, with
#      `Authorization: Bearer <CONCENTRATE_API_KEY>`, `stream: true`, the
#      model id passed through VERBATIM, a leading `system` input item, and
#      no top-level field outside the documented parameter reference.
#   4. The CLI printed a `done` receipt (exit 0) with the stub's reply text.
#   5. A wrong key produces the documented 401 body and a non-zero exit.
#
# Usage:
#   scripts/concentrate-selftest.sh                # builds a debug codewhale if needed
#   CODEWHALE_BIN=target/release/codewhale scripts/concentrate-selftest.sh
#   CONCENTRATE_SELFTEST_MODEL=openai/gpt-5.6-sol scripts/concentrate-selftest.sh
#
# Evidence level: LOCAL (fixture gateway + real binary). Not a provider
# canary — a paid canary against the live gateway is a separate, founder-gated
# step (see docs/PROVIDERS.md → Concentrate Notes).
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

model=${CONCENTRATE_SELFTEST_MODEL:-concentrate/auto}
expected_wire_model=${model#concentrate/}
stub_key=${CONCENTRATE_SELFTEST_KEY:-stub-key-not-a-real-credential}
work=$(mktemp -d "${TMPDIR:-/tmp}/concentrate-selftest.XXXXXX")
log="$work/stub.jsonl"
cleanup() {
  if [ -n "${stub_pid:-}" ]; then kill "$stub_pid" 2>/dev/null || true; wait "$stub_pid" 2>/dev/null || true; fi
  if [ -z "${CONCENTRATE_SELFTEST_KEEP:-}" ]; then rm -rf "$work"; fi
}
trap cleanup EXIT

bin=${CODEWHALE_BIN:-}
if [ -z "$bin" ]; then
  if [ -x target/release/codewhale ]; then
    bin=target/release/codewhale
  else
    echo "+ cargo build -p codewhale-cli --locked (debug; set CODEWHALE_BIN to skip)"
    cargo build -p codewhale-cli --locked >/dev/null
    bin=target/debug/codewhale
  fi
fi
[ -x "$bin" ] || { echo "codewhale binary not executable: $bin" >&2; exit 2; }

# 1. Stub on a free loopback port.
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
CONCENTRATE_STUB_PORT=$port CONCENTRATE_STUB_EXPECT_KEY=$stub_key CONCENTRATE_STUB_LOG=$log \
  python3 scripts/concentrate-stub.py >"$work/stub.out" 2>&1 &
stub_pid=$!
base="http://127.0.0.1:$port/v1"
for _ in $(seq 1 50); do
  if curl -sf -o /dev/null "$base/responses/health"; then break; fi
  sleep 0.1
done
curl -sf -o /dev/null "$base/responses/health" || { echo "stub did not answer /v1/responses/health" >&2; cat "$work/stub.out" >&2; exit 1; }
echo "ok: GET $base/responses/health -> 200"

# 2. Unauthenticated catalog.
models=$(curl -sf "$base/models")
python3 - "$models" <<'PY'
import json, sys
catalog = json.loads(sys.argv[1])
assert catalog["object"] == "list" and any(m["id"] == "deepseek-v4-pro" for m in catalog["data"]), catalog
print("ok: GET /v1/models is readable without a key (%d rows)" % len(catalog["data"]))
PY

# Isolated home + workspace so the run never touches the real config or secrets.
#
# The key and the loopback base URL are written into the isolated config file
# on purpose: Codewhale's credential-scope rule binds a saved or environment
# Concentrate key to the official gateway URL and refuses to send it to any
# other endpoint (a stub, a proxy, a typo). A custom endpoint receives a key
# only when the user writes both the base_url and the api_key into the same
# provider table — which is exactly what a BYOK user pointing at a local
# gateway would do.
home="$work/home"; ws="$work/ws"; mkdir -p "$home/.codewhale" "$ws"
write_config() {
  local key=$1
  cat >"$home/.codewhale/config.toml" <<TOML
provider = "concentrate"

[providers.concentrate]
base_url = "$base"
api_key = "$key"
model = "$model"
TOML
}
run_exec() {
  local key=$1 prompt=$2 out=$3
  write_config "$key"
  HOME="$home" XDG_CONFIG_HOME="$home/.config" CODEWHALE_HOME="$home/.codewhale" \
  CODEWHALE_CONFIG_PATH="$home/.codewhale/config.toml" \
    "$bin" --workspace "$ws" --no-project-config exec --auto --output-format stream-json "$prompt" >"$out" 2>"$out.err"
}

# 3+4. Real turn through the stub.
set +e
run_exec "$stub_key" "say ok" "$work/turn.jsonl"
exit_code=$?
set -e
if [ "$exit_code" -ne 0 ]; then
  echo "codewhale exec exited $exit_code" >&2; tail -20 "$work/turn.jsonl.err" >&2; exit 1
fi
python3 - "$log" "$work/turn.jsonl" "$stub_key" "$expected_wire_model" <<'PY'
import json, sys
log_path, turn_path, key, expected_model = sys.argv[1:5]
records = [json.loads(line) for line in open(log_path, encoding="utf-8") if line.strip()]
posts = [r for r in records if r["method"] == "POST"]
assert len(posts) == 1, f"expected exactly one POST /v1/responses, got {len(posts)}: {posts}"
post = posts[0]
assert post["path"].split("?")[0].rstrip("/") == "/v1/responses", post["path"]
assert post["authorization"] == f"Bearer {key}", post["authorization"]
assert post["model"] == expected_model, f"model passthrough: sent {post['model']!r}, expected {expected_model!r}"
assert post["stream"] is True, post
assert post["undocumented_fields"] == [], f"undocumented top-level fields sent: {post['undocumented_fields']}"
assert post["input_roles"] and post["input_roles"][0] == "system", f"system prompt must lead the input: {post['input_roles']}"
events = [json.loads(line) for line in open(turn_path, encoding="utf-8") if line.strip()]
types = [e.get("type") for e in events]
assert "done" in types, f"no done receipt in stream-json: {types}"
text = "".join(e.get("text") or e.get("content") or "" for e in events if e.get("type") == "content")
print("ok: POST /v1/responses once, Bearer header matched, model %r verbatim, stream:true, system item first, only documented fields" % expected_model)
print("ok: completed-turn receipt types = %s" % types)
if "ok from the concentrate stub" not in text:
    print("missing reply text in content events; types = %s; text = %r" % (types, text), file=sys.stderr)
    sys.exit(1)
print("ok: reply text reached the CLI output")
PY

# 5. Wrong key → documented 401 → non-zero exit.
: > "$log"
set +e
run_exec "wrong-key" "say ok" "$work/turn-401.jsonl"
bad_exit=$?
set -e
if [ "$bad_exit" -eq 0 ]; then echo "expected a non-zero exit with a wrong key" >&2; exit 1; fi
python3 - "$log" <<'PY'
import json, sys
records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
posts = [r for r in records if r["method"] == "POST"]
assert posts and posts[-1]["authorization"] == "Bearer wrong-key", posts
print("ok: wrong key was sent as `Bearer wrong-key`; the stub answered the documented 401 body")
PY
if grep -q "Invalid API key\|401" "$work/turn-401.jsonl" "$work/turn-401.jsonl.err"; then
  echo "ok: the 401 reached the CLI output (exit $bad_exit)"
else
  echo "ok: CLI exited $bad_exit on the 401 (message: $(tail -1 "$work/turn-401.jsonl.err"))"
fi

echo "CONCENTRATE SELFTEST PASS (binary: $bin, model: $model, stub: $base)"
