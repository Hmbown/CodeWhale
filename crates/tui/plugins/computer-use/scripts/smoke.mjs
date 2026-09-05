// Live smoke test: drives the real MCP server end-to-end on this machine.
// Policy: no destructive input (no clicks/typing into the user's session),
// no clipboard access, isolated state + recordings dirs.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import url from "node:url";

const __dirname = path.dirname(url.fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const results = [];

function log(name, pass, detail = "") {
  results.push({ name, pass, detail: String(detail).slice(0, 500) });
  console.log(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
}

const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-smoke-state-"));
const recDir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-smoke-rec-"));

const server = spawn("node", [path.join(ROOT, "mcp", "server.mjs")], {
  env: { ...process.env, CODEWHALE_CU_STATE_DIR: stateDir, CODEWHALE_CU_RECORDINGS_DIR: recDir },
  stdio: ["pipe", "pipe", "pipe"],
});
let buf = "";
const pending = new Map();
let nextId = 1;
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
      if (msg.id && pending.has(msg.id)) {
        pending.get(msg.id)(msg);
        pending.delete(msg.id);
      }
    } catch {}
  }
});
server.stderr.on("data", (d) => process.stderr.write(`[server] ${d}`));

function rpc(method, params, timeoutMs = 120_000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout waiting for ${method}`)); }, timeoutMs);
    pending.set(id, (msg) => { clearTimeout(t); resolve(msg); });
    server.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
}

async function tool(name, args = {}) {
  const res = await rpc("tools/call", { name, arguments: args });
  const text = res.result?.content?.[0]?.text ?? "{}";
  let parsed = null;
  try { parsed = JSON.parse(text); } catch {}
  return { raw: res.result, parsed, isError: res.result?.isError === true };
}

try {
  // --- protocol ---
  const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {} });
  log("initialize", !!init.result?.serverInfo?.name, `server=${init.result?.serverInfo?.name} v${init.result?.serverInfo?.version}`);
  await rpc("notifications/initialized", undefined, 5_000).catch(() => {});
  const tl = await rpc("tools/list", {});
  const tools = tl.result?.tools ?? [];
  log("tools/list", tools.length >= 38, `${tools.length} tools`);
  const names = new Set(tools.map((t) => t.name));
  for (const required of ["screenshot", "recording_start", "recording_stop", "computer_switch", "computer_list", "get_app_state", "left_click", "type", "key", "scroll", "zoom", "stop_computer_control"]) {
    if (!names.has(required)) log(`schema:${required}`, false, "missing");
  }
  log("schema:required-tools-present", true, "all key tools declared");

  // --- registry / switching ---
  let r = await tool("computer_list");
  log("computer_list", r.parsed?.ok === true && r.parsed?.computers?.length >= 1, `active=${r.parsed?.active}`);

  r = await tool("computer_register", { computer: "pad", transport: "hdc", label: "Harmony device" });
  log("computer_register(hdc)", r.parsed?.ok === true, JSON.stringify(r.parsed?.registered ?? r.parsed?.error));

  r = await tool("computer_switch", { computer: "pad" });
  log("computer_switch", r.parsed?.ok === true && r.parsed?.active === "pad", `active=${r.parsed?.active}`);

  r = await tool("list_apps");
  log("harmony fail-closed (no hdc device)", r.parsed?.ok === false, `code=${r.parsed?.error?.code}`);

  r = await tool("screenshot", { computer: "local" });
  log("switch-by-use (computer:local on screenshot)", r.parsed?.ok === true && r.parsed?.switched === true && r.parsed?.computer?.id === "local", `file=${path.basename(r.parsed?.file ?? "")}`);

  // --- local darwin live tools ---
  r = await tool("request_access");
  log("request_access/probe", r.parsed?.ok === true, `accessibility=${r.parsed?.permissions?.accessibility} capture=${r.parsed?.permissions?.screen_capture}`);

  r = await tool("list_displays");
  log("list_displays", r.parsed?.ok === true && r.parsed?.items?.length >= 1, JSON.stringify(r.parsed?.items?.[0] ?? r.parsed?.error));

  r = await tool("list_apps");
  log("list_apps", r.parsed?.ok === true && r.parsed?.apps?.length > 0, `${r.parsed?.apps?.length} apps`);

  const someApp = r.parsed?.apps?.find((a) => a.windowCount > 0);
  if (someApp) {
    const st = await tool("get_app_state", { app_ref: { pid: someApp.pid } });
    log("get_app_state", st.parsed?.ok === true && st.parsed?.elements?.length > 0, `${someApp.name}: ${st.parsed?.elements?.length} elements, state_id=${st.parsed?.state_id}`);
    globalThis.__state = st.parsed;
  } else {
    log("get_app_state", false, "no windowed app to observe");
  }

  r = await tool("cursor_position");
  log("cursor_position", r.parsed?.ok === true && Number.isFinite(r.parsed?.x), `x=${r.parsed?.x} y=${r.parsed?.y}`);

  r = await tool("screenshot", {});
  const shotFile = r.parsed?.file;
  const shotOk = r.parsed?.ok === true && shotFile && fs.existsSync(shotFile) && fs.statSync(shotFile).size > 0;
  log("screenshot (real file)", shotOk, `${shotFile ? path.basename(shotFile) : "?"} ${r.parsed?.bytes ?? 0}B scale=${r.parsed?.scale}`);

  if (shotOk) {
    const disp = r.parsed?.points;
    const w = Math.min(400, disp?.w ?? 400), h = Math.min(300, disp?.h ?? 300);
    r = await tool("zoom", { region: [0, 0, w, h] });
    log("zoom", r.parsed?.ok === true && r.parsed?.file && fs.existsSync(r.parsed.file), `${path.basename(r.parsed?.file ?? "")} ${r.parsed?.bytes ?? 0}B`);
  }

  // --- recording ---
  r = await tool("recording_start", {});
  const recId = r.parsed?.id;
  const started = r.parsed?.ok === true && !!recId;
  log("recording_start", started, `id=${recId} mode=${r.parsed?.mode} pid=${r.parsed?.pid}`);
  if (started) {
    await new Promise((res) => setTimeout(res, 2500));
    r = await tool("recording_status", { id: recId });
    log("recording_status", r.parsed?.ok === true && r.parsed?.running === true, `running=${r.parsed?.running} bytes-so-far=${r.parsed?.bytes} (screencapture flushes at stop)`);
    r = await tool("recording_stop", { id: recId });
    const stopped = r.parsed?.ok === true && r.parsed?.file && fs.existsSync(r.parsed.file) && fs.statSync(r.parsed.file).size > 1000;
    log("recording_stop (real file)", stopped, `${path.basename(r.parsed?.file ?? "")} + mp4:${r.parsed?.mp4 ? "yes" : "no"} bytes=${r.parsed?.bytes}`);
  }
  r = await tool("recording_list");
  log("recording_list", r.parsed?.ok === true && r.parsed?.recordings?.length >= 1, `${r.parsed?.recordings?.length} artifacts in ${r.parsed?.dir}`);

  // --- unknown tool + error shape ---
  r = await tool("no_such_tool");
  log("unknown tool fails closed", r.isError === true && r.parsed?.error?.code === "unknown_tool", `code=${r.parsed?.error?.code}`);

  // --- kill switch ---
  r = await tool("stop_computer_control", { reason: "smoke" });
  log("stop_computer_control", r.parsed?.ok === true && r.parsed?.stopped === true);
  r = await tool("list_apps");
  log("actions refused after kill switch", r.isError === true && r.parsed?.error?.code === "control_stopped", `code=${r.parsed?.error?.code}`);
  r = await tool("computer_list");
  log("read-only still allowed after kill switch", r.parsed?.ok === true);
} catch (err) {
  log("smoke-run", false, err.stack ?? err.message);
} finally {
  server.kill("SIGTERM");
  const receipt = { at: new Date().toISOString(), host: `${process.platform} ${os.release()}`, results };
  const outDir = path.join(ROOT, "receipts");
  fs.mkdirSync(outDir, { recursive: true });
  const out = path.join(outDir, `smoke-${new Date().toISOString().replace(/[:.]/g, "-")}.json`);
  fs.writeFileSync(out, JSON.stringify(receipt, null, 2));
  const failed = results.filter((r) => !r.pass);
  console.log(`\n${results.length - failed.length}/${results.length} passed. Receipt: ${out}`);
  try { fs.rmSync(stateDir, { recursive: true, force: true }); } catch {}
  process.exit(failed.length ? 1 : 0);
}
