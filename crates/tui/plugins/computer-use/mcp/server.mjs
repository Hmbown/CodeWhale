#!/usr/bin/env node
// codewhale-cu MCP server — zero-dependency JSON-RPC 2.0 over stdio.
// One tool surface, four platforms (darwin, win32, linux, harmonyos), with
// computer switching as a default: every tool accepts `computer`, and using a
// computer id switches the sticky active computer.
import * as registry from "../src/registry.mjs";
import { backendFor, installRemoteAgent, executorFor } from "../src/transport.mjs";
import { TOOLS, TOOL_NAMES, READ_ONLY_TOOLS, REMOTE_TOOLS, BACKEND_METHOD } from "../src/tools.mjs";
import { tryJson } from "../src/exec.mjs";

const VERSION = "0.1.0";
const SERVER_NAME = "codewhale-cu";

// ---------- per-session runtime state ----------
let controlStopped = false;
let stateCounter = 0;
/** state_id -> { computerId, app_ref, windowIndex, elements } */
const appStates = new Map();
/** computerId -> last raster metadata {file, scale, origin} */
const lastRasters = new Map();
/** computerId -> cached backend (local/hdc only) */
const backendCache = new Map();

function receipt(computer, extra) {
  return {
    computer: computer ? { id: computer.id, transport: computer.transport, platform: computer.platform ?? computer.platformHint ?? null } : null,
    ts: new Date().toISOString(),
    ...extra,
  };
}

function fail(computer, code, message, extra = {}) {
  return receipt(computer, { ok: false, error: { code, message }, ...extra });
}

async function getBackend(computer) {
  const key = computer.id;
  if (computer.transport === "local" || computer.transport === "hdc") {
    if (!backendCache.has(key)) {
      const { backend } = await backendFor(computer);
      backendCache.set(key, backend);
    }
    return backendCache.get(key);
  }
  const { backend } = await backendFor(computer);
  return backend;
}

/** Element target -> enriched target with cached app identity and AX path. */
function resolveElement(target) {
  const st = appStates.get(target.state_id);
  if (!st) throw new ServerError("unknown_state", `state_id "${target.state_id}" is unknown or expired — call get_app_state again`);
  const el = st.elements[target.index];
  if (!el) throw new ServerError("unknown_element", `element index ${target.index} is outside state ${target.state_id} (0..${st.elements.length - 1})`);
  return { state: st, element: el };
}

class ServerError extends Error {
  constructor(code, message) { super(message); this.code = code; }
}

/** Map raster-pixel coordinates to screen points using the bound raster. */
function rasterToPoints(computerId, x, y) {
  const r = lastRasters.get(computerId);
  if (!r) throw new ServerError("no_raster", "no screenshot bound on this computer yet — call screenshot first so pixel targets have a frame");
  const scale = r.scale && r.scale > 0 ? r.scale : 1;
  return { x: (r.origin?.x ?? 0) + x / scale, y: (r.origin?.y ?? 0) + y / scale };
}

/** Normalize a target into backend form: points for coordinates, resolved element for elements. */
function normalizeTarget(computer, target, kind) {
  if (target?.type === "coordinate") {
    const pt = rasterToPoints(computer.id, target.x, target.y);
    return { x: Math.round(pt.x), y: Math.round(pt.y), strategy: "event" };
  }
  if (target?.type === "element") {
    const { state, element } = resolveElement(target);
    if (kind === "semantic") {
      return {
        app_ref: state.app_ref, windowIndex: element.windowIndex ?? 0, path: element.path,
        strategy: "a11y", role: element.role, label: element.label,
      };
    }
    // Pointer tools on element targets: aim at the element center (cached geometry).
    if (!element.position || !element.size) throw new ServerError("element_no_geometry", `element ${target.index} has no cached geometry — use a coordinate target`);
    const c = { x: Math.round(element.position.x + element.size.w / 2), y: Math.round(element.position.y + element.size.h / 2) };
    return { ...c, strategy: "a11y-center", role: element.role, label: element.label };
  }
  throw new ServerError("bad_target", "target must be {type:'coordinate',x,y} or {type:'element',state_id,index}");
}

function bindRaster(computer, shot) {
  lastRasters.set(computer.id, {
    file: shot.file ?? shot.path,
    scale: shot.scale ?? 1,
    origin: shot.points ?? { x: 0, y: 0 },
    pixels: shot.pixels ?? null,
    capturedAt: shot.capturedAt ?? new Date().toISOString(),
  });
}

function rememberState(computer, app_ref, result) {
  const id = `s-${++stateCounter}`;
  appStates.set(id, { computerId: computer.id, app_ref, elements: result.elements ?? [], ts: Date.now() });
  if (appStates.size > 24) {
    for (const k of appStates.keys()) { appStates.delete(k); break; }
  }
  return id;
}

// ---------- tool dispatch ----------
async function callTool(params) {
  const name = params.name;
  if (!TOOL_NAMES.has(name)) {
    return { content: [{ type: "text", text: JSON.stringify({ ok: false, error: { code: "unknown_tool", message: `unknown tool "${name}"` } }) }], isError: true };
  }
  const args = params.arguments ?? {};

  if (name === "stop_computer_control") {
    controlStopped = true;
    return { content: [{ type: "text", text: JSON.stringify(receipt(null, { ok: true, stopped: true, note: "Computer control refused for the rest of this session. Restart the session or the codewhale-cu server to continue." })) }] };
  }
  if (controlStopped && !READ_ONLY_TOOLS.has(name)) {
    return { content: [{ type: "text", text: JSON.stringify(fail(null, "control_stopped", "stop_computer_control is active; no further actions are permitted this session")) }], isError: true };
  }

  if (name === "wait") {
    const s = Math.max(0, Math.min(30, Number(args.seconds) || 1));
    await new Promise((r) => setTimeout(r, s * 1000));
    return { content: [{ type: "text", text: JSON.stringify(receipt(null, { ok: true, waitedSec: s })) }] };
  }

  if (name === "computer_list") {
    const reg = registry.list();
    return { content: [{ type: "text", text: JSON.stringify(receipt(null, {
      ok: true,
      active: reg.active,
      computers: Object.values(reg.computers).map((c) => ({ id: c.id, transport: c.transport, platform: c.platform ?? c.platformHint ?? null, label: c.label ?? null, host: c.host ?? null })),
      note: "Pass `computer` on any tool to switch (sticky), or computer_switch to switch explicitly.",
    })) }] };
  }

  if (name === "computer_register") {
    try {
      const entry = registry.register({ id: args.computer, transport: args.transport, label: args.label, host: args.host, port: args.port, user: args.user, target: args.target });
      let installed = null;
      if (entry.transport === "ssh" && args.installAgent !== false) {
        installed = await installRemoteAgent(entry);
        registry.register({ id: entry.id, transport: "ssh", host: entry.host, port: entry.port, user: entry.user, platformHint: installed.remotePlatform, agentPath: installed.agentPath });
      }
      if (entry.transport === "ssh" && args.installAgent === false && !entry.platformHint) {
        // Probe cheaply through the agent; if it is missing, registration still succeeds.
        try {
          const ex = await executorFor(entry);
          const reply = await ex.remote({ tool: "platform" });
          registry.register({ id: entry.id, transport: "ssh", host: entry.host, port: entry.port, user: entry.user, platformHint: reply.platform });
        } catch {}
      }
      const fresh = registry.get(entry.id);
      return { content: [{ type: "text", text: JSON.stringify(receipt(null, { ok: true, registered: { ...fresh, platform: fresh.platform ?? fresh.platformHint ?? null }, agentInstall: installed })) }] };
    } catch (err) {
      // Registration problems (unreachable host, agent push failed) are
      // receipts, not protocol errors.
      return { content: [{ type: "text", text: JSON.stringify(fail(null, err.code ?? "register_failed", err.message ?? String(err))) }], isError: true };
    }
  }

  if (name === "computer_remove") {
    const res = registry.remove(args.computer);
    backendCache.delete(args.computer);
    lastRasters.delete(args.computer);
    return { content: [{ type: "text", text: JSON.stringify(receipt(null, { ok: true, ...res })) }] };
  }

  if (name === "computer_switch") {
    const c = registry.switchTo(args.computer);
    return { content: [{ type: "text", text: JSON.stringify(receipt(c, { ok: true, active: c.id })) }] };
  }

  // Everything below acts on a computer.
  let computer;
  let switched = false;
  try {
    if (args.computer && args.computer !== registry.list().active) {
      computer = registry.switchTo(args.computer);
      switched = true;
    } else {
      computer = registry.active();
    }
  } catch (err) {
    return { content: [{ type: "text", text: JSON.stringify(fail(null, err.code ?? "registry_error", err.message)) }], isError: true };
  }

  try {
    // ssh computers: dispatch over the wire to the remote agent.
    const backendMethod = BACKEND_METHOD[name] === "request_access" ? "probe" : BACKEND_METHOD[name];
    let data;

    if (computer.transport === "ssh" && REMOTE_TOOLS.has(backendMethod)) {
      const ex = await executorFor(computer);
      const wireArgs = prepareWireArgs(computer, name, args);
      const reply = await ex.remote({ tool: backendMethod, args: wireArgs }, { timeoutMs: backendMethod.startsWith("recording") || backendMethod === "get_app_state" ? 60_000 : 30_000 });
      if (!reply.ok) throw new ServerError(reply.error?.code ?? "remote_error", reply.error?.message ?? "remote agent failed");
      data = reply.data;
      if (Array.isArray(data)) data = { items: data };
      if (backendMethod === "screenshot" && data?.file) {
        // Raster lives on the remote machine; bind geometry for coordinate mapping.
        bindRaster(computer, { ...data, file: null });
        data.note = "file lives on the remote computer; pull it with scp if you need the bytes locally";
      }
    } else {
      const backend = await getBackend(computer);
      if (typeof backend[backendMethod] !== "function") {
        throw new ServerError("unsupported_on_backend", `"${name}" is not implemented on the ${computer.platform ?? computer.transport} backend`);
      }
      const prepared = prepareLocalArgs(computer, name, args);
      data = await backend[backendMethod](prepared);
      if (Array.isArray(data)) data = { items: data }; // keep receipts objects
      if (name === "screenshot") bindRaster(computer, data);
      if (name === "get_app_state") {
        const stateId = rememberState(computer, prepared.app_ref, data);
        data.state_id = stateId;
        data.note = "Element targets are {type:'element', state_id, index}. State goes stale when the UI changes; observe again.";
      }
    }

    return { content: [{ type: "text", text: JSON.stringify(receipt(computer, { ok: true, tool: name, switched, ...data })) }] };
  } catch (err) {
    return { content: [{ type: "text", text: JSON.stringify(fail(computer, err.code ?? "tool_error", err.message ?? String(err), { tool: name, switched })) }], isError: true };
  }
}

/** Convert public tool args into local-backend args (targets normalized). */
function prepareLocalArgs(computer, name, args) {
  const out = { ...args };
  delete out.computer;
  const semantic = new Set(["set_value", "select_text", "perform_action"]);
  if (out.target?.type) {
    out.target = normalizeTarget(computer, out.target, semantic.has(name) ? "semantic" : "pointer");
  }
  if (out.from_target?.type) out.from_target = normalizeTarget(computer, out.from_target, "pointer");
  if (out.to?.type) out.to = normalizeTarget(computer, out.to, "pointer");
  if (name === "get_app_state") {
    out.app_ref = out.app_ref ?? null;
    if (out.window_id != null) out.window_id = Number(out.window_id);
  }
  return out;
}

/** Convert public tool args into remote-agent args (self-contained, element targets resolved to paths). */
function prepareWireArgs(computer, name, args) {
  const out = { ...args };
  delete out.computer;
  const semantic = new Set(["set_value", "select_text", "perform_action"]);
  if (out.target?.type === "element") {
    const t = normalizeTarget(computer, out.target, semantic.has(name) ? "semantic" : "pointer");
    out.target = { ...out.target, app_ref: t.app_ref, windowIndex: t.windowIndex, path: t.path, x: t.x, y: t.y };
  }
  if (out.from_target?.type === "element") {
    const t = normalizeTarget(computer, out.from_target, "pointer");
    out.from_target = { ...out.from_target, x: t.x, y: t.y };
  }
  if (out.to?.type === "element") {
    const t = normalizeTarget(computer, out.to, "pointer");
    out.to = { ...out.to, x: t.x, y: t.y };
  }
  return out;
}

// ---------- JSON-RPC loop ----------
function respond(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
}
function respondError(id, code, message) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }) + "\n");
}

const HANDLERS = {
  initialize(params) {
    return {
      protocolVersion: params?.protocolVersion ?? "2025-06-18",
      capabilities: { tools: { listChanged: false } },
      serverInfo: { name: SERVER_NAME, version: VERSION, platforms: ["darwin", "win32", "linux", "harmonyos"], transports: ["local", "ssh", "hdc"] },
    };
  },
  "tools/list"() {
    return { tools: TOOLS };
  },
  async "tools/call"(params) {
    return await callTool(params ?? {});
  },
  ping() {
    return {};
  },
};

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let idx;
  while ((idx = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, idx).trim();
    buffer = buffer.slice(idx + 1);
    if (!line) continue;
    handleLine(line);
  }
});
process.stdin.on("end", () => process.exit(0));

async function handleLine(line) {
  const msg = tryJson(line, null);
  if (!msg || typeof msg !== "object") return;
  const { id, method, params } = msg;
  if (!method) return; // response to a server request — we never issue any
  const handler = HANDLERS[method];
  if (!handler) {
    if (id != null) respondError(id, -32601, `method not found: ${method}`);
    return;
  }
  try {
    const result = await handler(params);
    if (id != null) respond(id, result);
  } catch (err) {
    if (id != null) respondError(id, -32603, err?.message ?? String(err));
  }
}

// Notifications we must tolerate
["notifications/initialized", "initialized", "notifications/cancelled"].forEach((m) => { if (!HANDLERS[m]) HANDLERS[m] = () => ({}); });
