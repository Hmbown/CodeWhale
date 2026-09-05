// HarmonyOS backend — drives a device/emulator through `hdc` (HarmonyOS
// Device Connector) plus the on-device `uitest` and `snapshot_display` tools.
// Observation: `uitest dumpLayout` (the accessibility-tree equivalent).
// Input:       `uitest uiInput` (click / swipe / inputText / keyEvent).
// Stills:      `snapshot_display`. Video: no CLI screen recorder exists on
//              current HarmonyOS shells, so recording is an honest
//              snapshot-series mode muxed with ffmpeg on the host.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { run, runOk, ExecError, tryJson, have } from "../exec.mjs";

const DEVICE_TMP = "/data/local/tmp/cu";

function escDeviceText(s) {
  if (/[^\x20-\x7E]/.test(s)) throw new ExecError("harmony uiInput text must be printable ASCII on this backend");
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

function jpegSize(buf) {
  // Minimal JPEG SOF parser — enough to learn the panel size of a snapshot.
  let i = 2;
  while (i + 9 < buf.length) {
    if (buf[i] !== 0xff) { i++; continue; }
    const marker = buf[i + 1];
    if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
      return { h: buf.readUInt16BE(i + 5), w: buf.readUInt16BE(i + 7) };
    }
    i += 2 + buf.readUInt16BE(i + 2);
  }
  return null;
}

export function parseBounds(b) {
  const m = /\[(\d+),(\d+)\]\[(\d+),(\d+)\]/.exec(String(b ?? ""));
  if (!m) return null;
  const [x1, y1, x2, y2] = [Number(m[1]), Number(m[2]), Number(m[3]), Number(m[4])];
  return { x: x1, y: y1, w: x2 - x1, h: y2 - y1, cx: Math.round((x1 + x2) / 2), cy: Math.round((y1 + y2) / 2) };
}

export function flatten(node, pathArr = [], out = []) {
  if (!node || out.length >= 600) return out;
  const a = node.attributes ?? {};
  out.push({
    index: out.length,
    path: pathArr,
    role: a.type ?? "node",
    label: a.text || a.id || a.description || a.name || null,
    value: a.text ?? null,
    bounds: parseBounds(a.bounds),
    attributes: a,
    actions: ["click", "longClick", "inputText"],
  });
  for (let i = 0; i < (node.children?.length ?? 0); i++) flatten(node.children[i], [...pathArr, i], out);
  return out;
}

export function create({ exec }) {
  const shell = (args, opts = {}) => exec.shell(args, { timeoutMs: 20_000, ...opts });

  async function deviceOut(args, opts = {}) {
    const r = await shell(args, opts);
    if (r.code !== 0) throw new ExecError(`hdc shell ${args[0]} exited ${r.code}: ${(r.stderr || r.stdout).trim().slice(0, 300)}`, r);
    return r.stdout;
  }

  async function snapshot(localPath) {
    const remote = `${DEVICE_TMP}-${crypto.randomBytes(3).toString("hex")}.jpeg`;
    await deviceOut(["snapshot_display", "-f", remote], { timeoutMs: 25_000 });
    try {
      await exec.pullFile(remote, localPath, { timeoutMs: 30_000 });
    } finally {
      await shell(["rm", "-f", remote]).catch(() => {});
    }
    return localPath;
  }

  async function dumpLayout() {
    const remote = `${DEVICE_TMP}-layout.json`;
    await deviceOut(["uitest", "dumpLayout", "-p", remote], { timeoutMs: 40_000 });
    let data;
    try {
      data = await exec.readFile(remote, { timeoutMs: 30_000 });
    } finally {
      await shell(["rm", "-f", remote]).catch(() => {});
    }
    return JSON.parse(data.toString("utf8"));
  }

  async function uiInput(args, opts = {}) {
    await deviceOut(["uitest", "uiInput", ...args], opts);
    return { action_sent: true, strategy: "event", backend: "uitest" };
  }

  async function centerOf(target) {
    const tree = await dumpLayout();
    const els = flatten(tree);
    const el = els[target.index];
    if (!el || !el.bounds) throw new ExecError("element_stale — re-run get_app_state; uitest indexes change with the UI");
    return el.bounds;
  }

  let frameSeq = 0;
  let recording = null; // {id, dir, startedAt, intervalMs, timer, display}
  let displayPixels = null;

  return {
    platform: "harmonyos",
    probe: async () => {
      const r = await exec.run("hdc", [...exec.targetArgs, "list", "targets"], { timeoutMs: 10_000 });
      const targets = r.stdout.trim().split("\n").filter(Boolean);
      const connected = r.code === 0 && targets.length > 0 && !targets.includes("[Empty]");
      return {
        platform: "harmonyos",
        connected,
        targets,
        capabilities: { screenshot: connected, accessibility_tree: connected, clipboard: false, recording: "snapshot-series" },
        note: "HarmonyOS drives the device over hdc. Clipboard read/write is not exposed by hdc and fails closed. Recording muxes snapshot_display frames with ffmpeg.",
      };
    },
    list_displays: async () => {
      if (!displayPixels) {
        const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-hm-"));
        try {
          const shot = path.join(dir, "probe.jpeg");
          await snapshot(shot);
          displayPixels = jpegSize(fs.readFileSync(shot)) ?? { w: null, h: null };
        } finally { fs.rmSync(dir, { recursive: true, force: true }).catch(() => {}); }
      }
      return [{ index: 1, name: "device", pixels: displayPixels, points: displayPixels, scale: 1, main: true }];
    },
    async switch_display({ index }) {
      if (index !== 1) throw new ExecError("harmony backend exposes display 1 only");
      return { activeDisplay: 1 };
    },
    list_apps: async () => {
      const out = await deviceOut(["bm", "dump", "-a"], { timeoutMs: 25_000 });
      const bundles = out.split("\n").map((s) => s.trim()).filter((s) => /^[a-zA-Z][\w.]*$/.test(s));
      return { apps: bundles.map((b) => ({ name: b, bundle_id: b, kind: "bundle" })) };
    },
    list_windows: async () => {
      const out = await deviceOut(["hidumper", "-s", "WindowManagerService", "-a", "-a"], { timeoutMs: 25_000 }).catch(() => "");
      const windows = out.split("\n").filter((l) => /Window Name|bundleName/i.test(l)).slice(0, 40).map((l) => ({ title: l.trim().slice(0, 160) }));
      return { windows: windows.length ? windows : [{ title: "(window list unavailable on this HarmonyOS build)" }] };
    },
    open_application: async ({ bundle_id: bid, ability, name } = {}) => {
      const bundle = bid ?? name;
      if (!bundle) throw new ExecError("open_application needs bundle_id (harmony bundle name)");
      const candidates = ability ? [ability] : ["EntryAbility", "MainAbility"];
      let last = null;
      for (const a of candidates) {
        const r = await shell(["aa", "start", "-b", bundle, "-a", a]);
        if (r.code === 0 && !/Error|error/.test(r.stdout + r.stderr)) {
          return { launched: true, bundle, ability: a };
        }
        last = (r.stderr || r.stdout).trim().slice(0, 200);
      }
      throw new ExecError(`aa start failed: ${last}`);
    },
    get_app_state: async () => {
      const tree = await dumpLayout();
      const els = flatten(tree);
      return { bundle_id: tree.attributes?.bundleName ?? null, elements: els, truncated: els.length >= 600 };
    },
    screenshot: async ({ path: outPath } = {}) => {
      const dir = process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
      fs.mkdirSync(dir, { recursive: true });
      const file = outPath || path.join(dir, `shot-${new Date().toISOString().replace(/[:.]/g, "-")}-${crypto.randomBytes(3).toString("hex")}.jpeg`);
      await snapshot(file);
      const buf = fs.readFileSync(file);
      displayPixels = jpegSize(buf) ?? displayPixels;
      return { file, bytes: buf.length, pixels: jpegSize(buf), scale: 1, points: jpegSize(buf) };
    },
    zoom: async ({ region, path: outPath }) => {
      throw new ExecError("zoom is not supported on the harmony backend yet — screenshot + region on the host is the workaround");
    },
    left_click: ({ target }) => uiInput(["click", String(Math.round(target.x)), String(Math.round(target.y))]),
    double_click: ({ target }) => uiInput(["doubleClick", String(Math.round(target.x)), String(Math.round(target.y))]),
    triple_click: async ({ target }) => {
      await uiInput(["doubleClick", String(Math.round(target.x)), String(Math.round(target.y))]);
      return uiInput(["click", String(Math.round(target.x)), String(Math.round(target.y))]);
    },
    right_click: async () => { throw new ExecError("uitest uiInput has no right-click; use longClick semantics via hold or left_click"); },
    middle_click: async () => { throw new ExecError("middle click is not exposed by uitest uiInput"); },
    mouse_move: async () => ({ action_sent: false, note: "hover without press is not exposed by uitest uiInput" }),
    left_click_drag: ({ from_target: from, to }) =>
      uiInput(["swipe", String(Math.round(from.x)), String(Math.round(from.y)), String(Math.round(to.x)), String(Math.round(to.y)), "200"], { timeoutMs: 30_000 }),
    left_mouse_down: async () => { throw new ExecError("low-level press/release is not exposed by uitest uiInput; use left_click_drag"); },
    left_mouse_up: async () => { throw new ExecError("low-level press/release is not exposed by uitest uiInput; use left_click_drag"); },
    scroll: ({ target, direction = "down", amount = 300 }) => {
      const dist = Math.max(60, Math.min(1200, amount * 24));
      const dx = direction === "left" ? dist : direction === "right" ? -dist : 0;
      const dy = direction === "up" ? dist : direction === "down" ? -dist : 0;
      return uiInput(["swipe", String(Math.round(target.x)), String(Math.round(target.y)), String(Math.round(target.x + dx)), String(Math.round(target.y + dy)), "400"]);
    },
    type: async ({ text }) => {
      if (!text) return { action_sent: false, note: "empty text" };
      await uiInput(["inputText", "300", "300", escDeviceText(text)]).catch(async (e) => {
        // Some builds require coordinates of the focused field; retry with a click-first pattern.
        throw e;
      });
      return { action_sent: true, chars: text.length, note: "inputText at 300,300 — click the field first for focused input" };
    },
    key: ({ text }) => {
      const KEYMAP = { enter: "Enter", return: "Enter", escape: "Esc", esc: "Esc", back: "Back", home: "Home", backspace: "Back", delete: "Del", tab: "Tab", left: "DPAD_LEFT", right: "DPAD_RIGHT", up: "DPAD_UP", down: "DPAD_DOWN", power: "Power", menu: "Menu" };
      const k = KEYMAP[String(text).toLowerCase()] ?? String(text);
      if (!/^[A-Za-z0-9_]+$/.test(k)) throw new ExecError(`unsupported key "${text}" on harmony backend`);
      return uiInput(["keyEvent", k]);
    },
    hold_key: async ({ text, duration }) => {
      if (String(text).toLowerCase() !== "click") throw new ExecError('hold_key on harmony supports only hold_key({"text":"click"}) = longClick');
      const d = Math.max(1, Math.min(5, Number(duration) || 1));
      return uiInput(["longClick", "300", "300"]);
    },
    set_value: async ({ target, value }) => {
      const b = await centerOf(target);
      await uiInput(["click", String(b.cx), String(b.cy)]);
      await new Promise((r) => setTimeout(r, 300));
      await uiInput(["inputText", String(b.cx), String(b.cy), escDeviceText(String(value))]);
      return { action_sent: true, strategy: "uitest-element" };
    },
    select_text: async () => { throw new ExecError("select_text is not exposed by uitest dumpLayout/uiInput on the harmony backend"); },
    perform_action: async ({ target, action }) => {
      const b = await centerOf(target);
      if (action === "longClick") return uiInput(["longClick", String(b.cx), String(b.cy)]);
      return uiInput(["click", String(b.cx), String(b.cy)]);
    },
    read_clipboard: async () => { throw new ExecError("clipboard read is not exposed by hdc on current HarmonyOS builds"); },
    write_clipboard: async () => { throw new ExecError("clipboard write is not exposed by hdc on current HarmonyOS builds"); },
    cursor_position: async () => { throw new ExecError("cursor position does not exist on touch devices"); },
    recordingStart: async ({ intervalMs = 400 } = {}) => {
      if (recording) throw new ExecError(`recording ${recording.id} already running`);
      if (!(await have("ffmpeg"))) throw new ExecError("ffmpeg is required on the host to mux harmony snapshot-series recordings");
      const id = crypto.randomBytes(4).toString("hex");
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), `cu-rec-${id}-`));
      const startedAt = new Date().toISOString();
      recording = { id, dir, startedAt, intervalMs, seq: 0 };
      const tick = async () => {
        if (!recording || recording.id !== id) return;
        try {
          const remote = `${DEVICE_TMP}-rec-${String(recording.seq).padStart(5, "0")}.jpeg`;
          await deviceOut(["snapshot_display", "-f", remote], { timeoutMs: 15_000 });
          await exec.pullFile(remote, path.join(dir, `f${String(recording.seq).padStart(5, "0")}.jpeg`), { timeoutMs: 15_000 });
          await shell(["rm", "-f", remote]).catch(() => {});
          recording.seq++;
        } catch {}
      };
      await tick();
      const timer = setInterval(tick, Math.max(150, intervalMs));
      recording.timer = timer;
      return { id, mode: "snapshot-series", intervalMs, startedAt, note: "no HarmonyOS CLI screen recorder; frames are muxed into mp4 on stop" };
    },
    recordingStop: async ({ id }) => {
      if (!recording || recording.id !== id) throw new ExecError(`unknown recording "${id}"`);
      clearInterval(recording.timer);
      const { dir, seq, startedAt, intervalMs } = recording;
      recording = null;
      const dirOut = process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
      fs.mkdirSync(dirOut, { recursive: true });
      const out = path.join(dirOut, `rec-${id}.mp4`);
      const fps = Math.max(1, Math.min(15, Math.round(1000 / Math.max(150, intervalMs))));
      const r = await run("ffmpeg", ["-y", "-loglevel", "error", "-framerate", String(fps), "-i", path.join(dir, "f%05d.jpeg"), "-c:v", "libx264", "-pix_fmt", "yuv420p", out], { timeoutMs: 180_000 });
      const bytes = fs.existsSync(out) ? fs.statSync(out).size : 0;
      try { fs.rmSync(dir, { recursive: true, force: true }); } catch {}
      if (r.code !== 0) throw new ExecError(`ffmpeg mux failed: ${(r.stderr || "").slice(0, 300)}`, r);
      return { id, mode: "snapshot-series", frames: seq, fps, file: out, bytes, startedAt, stoppedAt: new Date().toISOString() };
    },
    recordingStatus: ({ id }) => recording && recording.id === id
      ? { id, running: true, mode: "snapshot-series", frames: recording.seq, startedAt: recording.startedAt }
      : { id, running: false },
    recordingList: async () => {
      const dir = process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
      const out = fs.existsSync(dir) ? fs.readdirSync(dir).filter((f) => /\.(mp4|mov|jpeg|png)$/i.test(f)).map((f) => {
        const st = fs.statSync(path.join(dir, f));
        return { file: path.join(dir, f), bytes: st.size, modifiedAt: st.mtime.toISOString() };
      }).sort((a, b) => b.modifiedAt.localeCompare(a.modifiedAt)).slice(0, 50) : [];
      return { dir, recordings: out, running: recording ? [recording.id] : [] };
    },
  };
}

export default { create };
