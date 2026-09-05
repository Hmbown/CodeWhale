// Server protocol tests: real MCP server process over stdio, isolated state.
// The ssh/scp shims stand in for a remote machine, proving the full remote
// agent loop (install -> platform probe -> tool dispatch) without real ssh.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import url from "node:url";

const __dirname = path.dirname(url.fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");

const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-proto-state-"));
const recDir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-proto-rec-"));
const fakeHome = fs.mkdtempSync(path.join(os.tmpdir(), "cu-proto-home-"));
const binDir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-proto-bin-"));

// Fake ssh: rebuild the remote command after user@host, then either run the
// agent or emulate the one remote command the installer needs (mkdir -p).
fs.writeFileSync(path.join(binDir, "ssh"), `#!/bin/bash
CMD=()
FOUND=0
for a in "$@"; do
  if [ "$FOUND" -eq 1 ]; then CMD+=("$a"); fi
  case "$a" in *@*) [ "$FOUND" -eq 0 ] && FOUND=1 ;; esac
done
SUB="\${CMD[0]}"
if [ "$SUB" = "node" ]; then
  exec node "$FAKE_HOME/\${CMD[1]}" "\${CMD[2]}"
fi
if [ "$SUB" = "mkdir" ]; then
  LAST="\${CMD[\${#CMD[@]}-1]}"
  mkdir -p "$FAKE_HOME/$LAST"
  exit 0
fi
exit 0
`);
// Fake scp: copies <src> to <user@host:dest> under FAKE_HOME.
fs.writeFileSync(path.join(binDir, "scp"), `#!/bin/bash
SRC="$(printf '%s\\n' "$@" | tail -n 2 | head -n 1)"
DEST="$(printf '%s\\n' "$@" | tail -n 1)"
DEST="$FAKE_HOME/\${DEST#*:}"
mkdir -p "$(dirname "$DEST")"
cp "$SRC" "$DEST"
`);
fs.chmodSync(path.join(binDir, "ssh"), 0o755);
fs.chmodSync(path.join(binDir, "scp"), 0o755);

let server;
let buf = "";
const pending = new Map();
let nextId = 1;

function rpc(method, params, timeoutMs = 90_000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout: ${method}`)); }, timeoutMs);
    pending.set(id, (msg) => { clearTimeout(t); resolve(msg); });
    server.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
}

async function tool(name, args = {}) {
  const res = await rpc("tools/call", { name, arguments: args });
  assert.ok(res.result, `${name}: protocol error ${JSON.stringify(res.error ?? {})}`);
  return JSON.parse(res.result.content[0].text);
}

before(async () => {
  server = spawn("node", [path.join(ROOT, "mcp", "server.mjs")], {
    env: {
      ...process.env,
      PATH: `${binDir}:${process.env.PATH}`,
      FAKE_HOME: fakeHome,
      CODEWHALE_CU_STATE_DIR: stateDir,
      CODEWHALE_CU_RECORDINGS_DIR: recDir,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  server.stderr.on("data", (d) => process.stderr.write(`[server] ${d}`));
  server.stdout.setEncoding("utf8");
  server.stdout.on("data", (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (!line) continue;
      try {
        const msg = JSON.parse(line);
        if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
      } catch {}
    }
  });
  const init = await rpc("initialize", { protocolVersion: "2025-06-18" });
  assert.equal(init.result.serverInfo.name, "codewhale-cu");
});

after(() => {
  server?.kill("SIGTERM");
  for (const d of [stateDir, recDir, fakeHome]) { try { fs.rmSync(d, { recursive: true, force: true }); } catch {} }
});

test("tools/list exposes the full frontier surface with valid schemas", async () => {
  const res = await rpc("tools/list", {});
  const tools = res.result.tools;
  assert.ok(tools.length >= 38, `${tools.length} tools`);
  for (const t of tools) {
    assert.ok(t.name && t.description && t.inputSchema, `schema incomplete for ${t.name}`);
  }
  const names = new Set(tools.map((t) => t.name));
  for (const required of ["screenshot", "zoom", "left_click", "double_click", "triple_click", "right_click", "middle_click",
    "mouse_move", "left_click_drag", "left_mouse_down", "left_mouse_up", "scroll", "type", "key", "hold_key",
    "set_value", "select_text", "perform_action", "get_app_state", "list_apps", "list_windows", "list_displays",
    "switch_display", "open_application", "read_clipboard", "write_clipboard", "cursor_position", "wait",
    "recording_start", "recording_stop", "recording_status", "recording_list",
    "computer_list", "computer_switch", "computer_register", "computer_remove", "request_access", "stop_computer_control"]) {
    assert.ok(names.has(required), `missing tool ${required}`);
  }
});

test("computer registry round-trip over the protocol", async () => {
  let r = await tool("computer_list");
  assert.equal(r.ok, true);
  assert.equal(r.active, "local");
  r = await tool("computer_register", { computer: "pad", transport: "hdc" });
  assert.equal(r.registered.platform, "harmonyos");
  r = await tool("computer_switch", { computer: "pad" });
  assert.equal(r.active, "pad");
  r = await tool("computer_remove", { computer: "pad" });
  assert.equal(r.active, "local");
});

test("registering an ssh computer installs the agent and probes the platform", async () => {
  const r = await tool("computer_register", { computer: "box", transport: "ssh", host: "box.test", user: "me" });
  assert.equal(r.ok, true, JSON.stringify(r.error ?? {}));
  assert.equal(r.agentInstall.remotePlatform, process.platform, "platform probed via agent");
  assert.ok(fs.existsSync(path.join(fakeHome, ".codewhale-cu", "agent", "agent.mjs")), "agent pushed");
  assert.ok(fs.existsSync(path.join(fakeHome, ".codewhale-cu", "agent", "src", "backends", "darwin.mjs")), "src tree pushed");
  // dispatch a real tool to the "remote" computer. A headless Linux host
  // (CI) has no window manager tooling, so the remote backend fails closed
  // with its named reason; that error still proves the round trip.
  const apps = await tool("list_apps", { computer: "box" });
  if (apps.ok) {
    assert.equal(apps.computer.id, "box");
    assert.ok(Array.isArray(apps.apps) && apps.apps.length > 0, "apps returned over the wire");
  } else {
    assert.equal(process.platform, "linux", JSON.stringify(apps.error ?? {}));
    assert.equal(apps.error.code, "tool_error");
    assert.match(apps.error.message, /wmctrl|swaymsg|hyprctl/u);
  }
});

test("unknown computer fails closed with a named error", async () => {
  const r = await tool("screenshot", { computer: "ghost" });
  assert.equal(r.ok, false);
  assert.equal(r.error.code, "unknown_computer");
});

test("kill switch refuses mutating tools but keeps read-only probes", async () => {
  let r = await tool("stop_computer_control", { reason: "protocol-test" });
  assert.equal(r.stopped, true);
  r = await tool("screenshot");
  assert.equal(r.error.code, "control_stopped");
  r = await tool("computer_list");
  assert.equal(r.ok, true);
});
