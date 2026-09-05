// macOS backend. Zero third-party dependencies:
//  - observation:  osascript JXA over System Events (accessibility tree)
//  - raw input:    CGEvent posted through JXA's CoreGraphics bridge
//  - stills/video: /usr/sbin/screencapture  (video requires macOS 13+)
//  - crop:         sips   - clipboard: pbcopy/pbpaste
// All scripts travel as temp files + one base64 payload argument, so tool
// arguments never become AppleScript syntax.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawn } from "node:child_process";
import { run, runOk, ExecError, tryJson, have } from "../exec.mjs";

const KEY_CODES = {
  return: 36, enter: 36, tab: 48, space: 49, escape: 53, esc: 53, delete: 51,
  backspace: 51, forwarddelete: 117, home: 115, end: 119, pageup: 116, pagedown: 121,
  left: 123, right: 124, down: 125, up: 126, clear: 71, capslock: 57, f1: 122,
  f2: 120, f3: 99, f4: 118, f5: 96, f6: 97, f7: 98, f8: 100, f9: 101, f10: 109,
  f11: 103, f12: 111, volumeup: 72, volumedown: 73, mute: 74, help: 114,
  a: 0, s: 1, d: 2, f: 3, h: 4, g: 5, z: 6, x: 7, c: 8, v: 9, b: 11, q: 12,
  w: 13, e: 14, r: 15, y: 16, t: 17, "1": 18, "2": 19, "3": 20, "4": 21,
  "5": 23, "6": 22, "7": 26, "8": 28, "9": 25, "0": 29, "-": 27, "=": 24,
  "[": 33, "]": 30, "\\": 42, ";": 41, "'": 39, ",": 43, ".": 47, "/": 44,
  o: 31, u: 32, i: 34, p: 35, l: 37, j: 38, k: 40, n: 45, m: 46,
};
const MODIFIERS = {
  cmd: 1 << 20, command: 1 << 20, win: 1 << 20, meta: 1 << 20,
  shift: 1 << 17, ctrl: 1 << 18, control: 1 << 18, alt: 1 << 19, opt: 1 << 19, option: 1 << 19,
  fn: 1 << 23, function: 1 << 23,
};
const MOUSE = {
  left: { down: 1, up: 2, dragged: 7 },
  right: { down: 3, up: 4, dragged: 8 },
  middle: { down: 25, up: 26, dragged: 27 },
};

export function create({ exec }) {
  const runL = (cmd, args, opts) => exec.run(cmd, args, opts);
  const state = { activeDisplay: 1, lastRaster: null };

  // ---------- JXA helper ----------
  async function jxa(script, payload = {}, timeoutMs = 20_000) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-jxa-"));
    try {
      const file = path.join(dir, "s.js");
      fs.writeFileSync(file, script);
      // Payload travels as a plain argv string: osascript is spawned without a
      // shell, so JSON content can never become script syntax.
      const r = await runL("osascript", ["-l", "JavaScript", file, JSON.stringify(payload)], { timeoutMs });
      if (r.timedOut) throw new ExecError("osascript timed out", r);
      if (r.code !== 0) {
        const msg = (r.stderr || r.stdout).trim().split("\n")[0] || "osascript failed";
        throw new ExecError(/(not allowed assistive|assistive access|250)/i.test(r.stderr || "") || /(-25211|-1719|not allowed)/i.test(msg)
          ? `${msg} (accessibility permission for the host terminal is required: System Settings → Privacy & Security → Accessibility)`
          : msg, r);
      }
      return tryJson(r.stdout.trim(), r.stdout.trim());
    } finally {
      try { fs.rmSync(dir, { recursive: true, force: true }); } catch {}
    }
  }

  const JXA_PRELUDE = `
    function run(argv){
      var P = JSON.parse(argv[0]);
      function g(f){ try { return f(); } catch(e){ return null; } }
      function num(v){ if (typeof v==='function') v = g(v); if (v===null||v===undefined) return null; if (typeof v==='object'){ try { v = ObjC.unwrap(v); } catch(e){} } var n = Number(v); return isFinite(n)? n : null; }
      function pt(p){ p = g(function(){return p();}); if(!p) return null; try { return { x: num(p[0]!==undefined?p[0]:p.x), y: num(p[1]!==undefined?p[1]:p.y) }; } catch(e){ return null; } }
      function sz(s){ s = g(function(){return s();}); if(!s) return null; try { return { w: num(s[0]!==undefined?s[0]:s.width), h: num(s[1]!==undefined?s[1]:s.height) }; } catch(e){ return null; } }
      function elInfo(el, idx, winIdx, path){
        return {
          index: idx, path: path, windowIndex: winIdx,
          role: g(function(){ return String(el.role()); }),
          subrole: g(function(){ var s = el.subrole(); return s ? String(s) : null; }),
          label: g(function(){ var t = el.title(); if(t) return String(t); var n = el.name(); if(n) return String(n); var h = el.help(); return h ? String(h) : null; }),
          value: g(function(){ var v = el.value(); if (v===null||v===undefined) return null; var s = String(v); return s.length>120? s.slice(0,120)+'…' : s; }),
          enabled: g(function(){ return !!el.enabled(); }),
          focused: g(function(){ return !!el.focused(); }),
          position: pt(function(){ return el.position(); }),
          size: sz(function(){ return el.size(); }),
          actions: g(function(){ return el.actions().map(function(a){ return String(a.name()); }); }) || []
        };
      }
  `;

  async function findProcess(ref) {
    return jxa(`${JXA_PRELUDE}
      var se = Application('System Events');
      var procs = se.applicationProcesses.js ? se.applicationProcesses : se.applicationProcesses;
      var found = null;
      var list = se.applicationProcesses();
      for (var i=0;i<list.length;i++){
        var p = list[i];
        if (P.pid != null && num(function(){ return p.unixId(); }) === P.pid){ found = p; break; }
        if (P.bundle_id){ var b = g(function(){ return String(p.bundleIdentifier()); }); if (b && b.toLowerCase()===P.bundle_id.toLowerCase()){ found = p; break; } }
        if (P.name){ var n = g(function(){ return String(p.name()); }); if (n && n.toLowerCase()===P.name.toLowerCase()){ found = p; break; } }
      }
      if (!found) { return JSON.stringify({found:false}); }
      else {
        return JSON.stringify({ found: true, name: g(function(){return String(found.name());}), pid: num(function(){return found.unixId();}),
          bundle_id: g(function(){ var b = found.bundleIdentifier(); return b? String(b): null; }),
          frontmost: g(function(){ return !!found.frontmost(); }),
          windows: (function(){ var out=[]; var ws = g(function(){ return found.windows(); }) || [];
            for (var i=0;i<ws.length;i++){ out.push({ index:i, title: g(function(){ var t = ws[i].name(); return t? String(t): null; })(),
              subrole: g(function(){ var s = ws[i].subrole(); return s? String(s): null; })(),
              position: pt(function(){ return ws[i].position(); }), size: sz(function(){ return ws[i].size(); }) }); }
            return out; })() });
      }
    }`, ref);
  }

  async function walkTree(appRef, depth = 8, maxElements = 400) {
    return jxa(`${JXA_PRELUDE}
      var se = Application('System Events');
      var procs = se.applicationProcesses();
      var found = null;
      for (var i=0;i<procs.length;i++){
        var p = procs[i];
        if (P.pid != null && num(function(){ return p.unixId(); }) === P.pid){ found = p; break; }
        if (P.bundle_id){ var b = g(function(){ return String(p.bundleIdentifier()); }); if (b && b.toLowerCase()===P.bundle_id.toLowerCase()){ found = p; break; } }
        if (P.name){ var n = g(function(){ return String(p.name()); }); if (n && n.toLowerCase()===P.name.toLowerCase()){ found = p; break; } }
      }
      if (!found) { return JSON.stringify({found:false}); }
      else {
        var elements = [];
        var truncated = false;
        function descend(el, winIdx, path, d){
          if (elements.length >= P.maxElements || d > P.depth){ if (d > P.depth) truncated = true; return; }
          var info = elInfo(el, elements.length, winIdx, path);
          elements.push(info);
          var kids = g(function(){ return el.uiElements(); }) || [];
          for (var k=0;k<kids.length;k++) descend(kids[k], winIdx, path.concat(k), d+1);
        }
        var ws = g(function(){ return found.windows(); }) || [];
        var winLimit = (P.window_id != null) ? [P.window_id] : null;
        for (var w=0; w<ws.length; w++){
          if (winLimit && winLimit.indexOf(w) === -1) continue;
          descend(ws[w], w, [], 0);
        }
        return JSON.stringify({ found: true,
          name: g(function(){return String(found.name());}), pid: num(function(){return found.unixId();}),
          bundle_id: g(function(){ var b=found.bundleIdentifier(); return b? String(b): null; }),
          frontmost: g(function(){ return !!found.frontmost(); }),
          truncated: truncated, elements: elements });
      }
    }`, { ...appRef, depth, maxElements }, 30_000);
  }

  async function resolveElementPath(appRef, windowIndex, pathArr) {
    return jxa(`${JXA_PRELUDE}
      var se = Application('System Events');
      var procs = se.applicationProcesses();
      var found = null;
      for (var i=0;i<procs.length;i++){
        var p = procs[i];
        if (P.pid != null && num(function(){ return p.unixId(); }) === P.pid){ found = p; break; }
        if (P.bundle_id){ var b = g(function(){ return String(p.bundleIdentifier()); }); if (b && b.toLowerCase()===P.bundle_id.toLowerCase()){ found = p; break; } }
        if (P.name){ var n = g(function(){ return String(p.name()); }); if (n && n.toLowerCase()===P.name.toLowerCase()){ found = p; break; } }
      }
      if (!found) { return JSON.stringify({found:false}); }
      else {
        var ws = g(function(){ return found.windows(); }) || [];
        if (!(P.windowIndex >= 0) || P.windowIndex >= ws.length){ return JSON.stringify({found:true, element:false, reason:"window_index_missing"}); }
        else {
          var el = ws[P.windowIndex]; var ok = true;
          for (var k=0;k<P.path.length;k++){
            var kids = g(function(){ return el.uiElements(); }) || [];
            if (P.path[k] >= kids.length){ ok = false; break; }
            el = kids[P.path[k]];
          }
          if (!ok) return JSON.stringify({found:true, element:false, reason:"element_stale"});
          else {
            var info = elInfo(el, 0, P.windowIndex, P.path);
            info.app = { name: g(function(){return String(found.name());}), pid: num(function(){return found.unixId();}) };
            return JSON.stringify({ found:true, element:true, element: info });
          }
        }
      }
    }`, { ...appRef, windowIndex, path: pathArr }, 30_000);
  }

  async function elementAction(appRef, windowIndex, pathArr, action) {
    return jxa(`${JXA_PRELUDE}
      var se = Application('System Events');
      var procs = se.applicationProcesses();
      var found = null;
      for (var i=0;i<procs.length;i++){
        var p = procs[i];
        if (P.pid != null && num(function(){ return p.unixId(); }) === P.pid){ found = p; break; }
        if (P.bundle_id){ var b = g(function(){ return String(p.bundleIdentifier()); }); if (b && b.toLowerCase()===P.bundle_id.toLowerCase()){ found = p; break; } }
        if (P.name){ var n = g(function(){ return String(p.name()); }); if (n && n.toLowerCase()===P.name.toLowerCase()){ found = p; break; } }
      }
      if (!found) { return JSON.stringify({ok:false, code:"app_not_found"}); }
      else {
        var ws = g(function(){ return found.windows(); }) || [];
        if (!(P.windowIndex >= 0) || P.windowIndex >= ws.length){ return JSON.stringify({ok:false, code:"element_stale"}); }
        else {
          var el = ws[P.windowIndex]; var ok = true;
          for (var k=0;k<P.path.length;k++){
            var kids = g(function(){ return el.uiElements(); }) || [];
            if (P.path[k] >= kids.length){ ok = false; break; }
            el = kids[P.path[k]];
          }
          if (!ok){ return JSON.stringify({ok:false, code:"element_stale"}); }
          else {
            var done = false, err = null;
            try {
              if (P.kind === 'action'){ el.actions.byName(P.action).perform(); done = true; }
              else if (P.kind === 'set_value'){ el.value = P.value; done = true; }
              else if (P.kind === 'select_text'){
                el.attributes.byName('AXSelectedTextRange').value = { loc: P.range[0], len: P.range[1] };
                done = true;
              }
              else { err = 'unknown_kind'; }
            } catch(e){ err = String(e); }
            return JSON.stringify({ ok: done && !err, code: err || 'sent', sent: done,
              element: { role: g(function(){ return String(el.role()); })(), label: g(function(){ var t=el.title(); return t?String(t):(el.name()?String(el.name()):null); })() } });
          }
        }
      }
    }`, { ...appRef, windowIndex, path: pathArr, kind: action.kind, action: action.action, value: action.value, range: action.range }, 30_000);
  }

  // ---------- raw input via CGEvent ----------
  async function cg(script, timeoutMs = 10_000) {
    return jxa(`ObjC.import('CoreGraphics');
      function run(argv){ var P = JSON.parse(argv[0]);
        ${script}
      }`, {}, timeoutMs);
  }

  async function postMouseEvent(type, x, y, button, clickState) {
    return cg(`var pt = { x: P.x, y: P.y };
      var ev = $.CGEventCreateMouseEvent($(), P.type, pt, P.button);
      if (P.clickState > 1) $.CGEventSetIntegerValueField(ev, $.kCGMouseEventClickState, P.clickState);
      $.CGEventPost($.kCGHIDEventTap, ev);
      return JSON.stringify({ ok: true, x: P.x, y: P.y, type: P.type, button: P.button });`,
      { type, x, y, button, clickState });
  }

  function mouseName(button) { return { left: "left", right: "right", middle: "middle" }[button] ?? "left"; }

  function assertInScreen(x, y) {
    if (!Number.isFinite(x) || !Number.isFinite(y)) throw new ExecError("coordinates must be finite numbers");
  }

  async function pointerClick(button, x, y, clicks) {
    assertInScreen(x, y);
    const m = MOUSE[button] ?? MOUSE.left;
    await postMouseEvent(m.dragged, x, y, button === "middle" ? 2 : button === "right" ? 1 : 0, clicks);
    await postMouseEvent(m.down, x, y, button === "middle" ? 2 : button === "right" ? 1 : 0, clicks);
    await postMouseEvent(m.up, x, y, button === "middle" ? 2 : button === "right" ? 1 : 0, clicks);
    return { action_sent: true, strategy: "event", at: { x, y }, button, clicks };
  }

  async function keyEvent(code, flags, down) {
    return cg(`var ev = $.CGEventCreateKeyboardEvent($(), P.code, P.down);
      if (P.flags) $.CGEventSetFlags(ev, P.flags);
      $.CGEventPost($.kCGHIDEventTap, ev);
      return JSON.stringify({ ok: true, code: P.code, down: P.down });`, { code, flags, down });
  }

  function parseChord(text) {
    const parts = String(text).split("+").map((s) => s.trim().toLowerCase()).filter(Boolean);
    if (!parts.length) throw new ExecError("empty key text");
    let flags = 0;
    let key = null;
    for (const p of parts) {
      if (MODIFIERS[p] != null) flags |= MODIFIERS[p];
      else if (KEY_CODES[p] != null) { if (key) throw new ExecError(`multiple non-modifier keys in "${text}"`); key = p; }
      else throw new ExecError(`unknown key "${p}" (supported: ${Object.keys(KEY_CODES).join(", ")} + modifiers cmd/ctrl/alt/shift/fn)`);
    }
    if (key == null) throw new ExecError(`no non-modifier key in "${text}" — use hold_key for modifier-only holds`);
    return { flags, code: KEY_CODES[key], key };
  }

  // ---------- displays ----------
  async function displayInfo() {
    const r = await runL("system_profiler", ["SPDisplaysDataType", "-json"], { timeoutMs: 25_000 });
    const j = tryJson(r.stdout, null);
    const items = j?.SPDisplaysDataType?.flatMap?.((g) => g.spdisplays_ndrvs ?? []) ?? [];
    // Main-display point geometry via Finder (pure AppleScript — JXA cannot
    // bridge C functions that return structs like CGRect).
    const bounds = await runL("osascript", ["-e", 'tell application "Finder" to get bounds of window of desktop'], { timeoutMs: 12_000 }).then((x) =>
      x.code === 0 ? x.stdout.trim().split(",").map((n) => Number(n.trim())) : null).catch(() => null);
    return items.map((d, i) => {
      const res = (d._spdisplays_resolution ?? "").match(/(\d+)\s*x\s*(\d+)/) ?? [null, null, null];
      return {
        index: i + 1,
        id: d._spdisplays_display_id ?? null,
        name: d._name ?? `Display ${i + 1}`,
        pixels: { w: Number(res[1]) || null, h: Number(res[2]) || null },
        main: String(d.spdisplays_main ?? "n").toLowerCase() === "y" || i === 0,
      };
    }).map((d, i) => ({
      ...d,
      // Point geometry is only precisely known for the main display; other
      // displays get best-effort placement to the right of the main display.
      points: i === 0 && bounds && bounds.length === 4
        ? { x: bounds[0], y: bounds[1], w: bounds[2] - bounds[0], h: bounds[3] - bounds[1] }
        : { x: null, y: null, w: null, h: null },
      scale: d.pixels.w && i === 0 && bounds?.length === 4 && bounds[2] - bounds[0] > 0
        ? +(d.pixels.w / (bounds[2] - bounds[0])).toFixed(3) : 1,
    }));
  }

  // ---------- screenshots ----------
  function recordingsDir() {
    return process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
  }

  async function screenshot({ display, region, path: outPath } = {}) {
    const dir = recordingsDir();
    fs.mkdirSync(dir, { recursive: true });
    const file = outPath || path.join(dir, `shot-${new Date().toISOString().replace(/[:.]/g, "-")}-${crypto.randomBytes(3).toString("hex")}.png`);
    if (!/\.png$/.test(file)) throw new ExecError("screenshot path must end in .png");
    const args = ["-x", "-t", "png"];
    const disp = display ?? state.activeDisplay;
    if (disp && disp !== "all") args.push("-D", String(disp));
    if (region) {
      if (!region.every((n) => Number.isFinite(n) && n >= 0) || region.length !== 4) {
        throw new ExecError("region must be [x, y, w, h] in screen points");
      }
      args.push("-R", region.join(","));
    }
    args.push(file);
    const r = await runL("screencapture", args, { timeoutMs: 20_000 });
    if (r.code !== 0) throw new ExecError(`screencapture exited ${r.code}: ${r.stderr.trim().slice(0, 300)}`, r);
    const stat = fs.statSync(file);
    const displays = await displayInfo();
    const d = displays.find((x) => x.index === (disp === "all" ? 1 : disp)) ?? displays[0];
    state.lastRaster = {
      file,
      bytes: stat.size,
      display: disp ?? 1,
      points: d?.points ?? null,
      pixels: d?.pixels ?? null,
      scale: d?.scale ?? 1,
      capturedAt: new Date().toISOString(),
    };
    return { ...state.lastRaster, path: file };
  }

  async function zoom({ source, region, path: outPath }) {
    if (!source && !state.lastRaster) throw new ExecError("no screenshot taken yet on this computer — call screenshot first");
    const [x, y, w, h] = region;
    if (![x, y, w, h].every((n) => Number.isFinite(n) && n >= 0)) throw new ExecError("region must be [x, y, w, h] in last-raster pixels");
    const src = source ?? state.lastRaster.file;
    const dir = recordingsDir();
    fs.mkdirSync(dir, { recursive: true });
    const out = outPath || path.join(dir, `zoom-${crypto.randomBytes(4).toString("hex")}.png`);
    await runOk("sips", ["-s", "format", "png", "-c", String(Math.round(h)), String(Math.round(w)), "--cropOffset", String(Math.round(y)), String(Math.round(x)), src, "--out", out], { timeoutMs: 15_000 });
    return { file: out, bytes: fs.statSync(out).size, source: src, region, scale: state.lastRaster.scale };
  }

  // ---------- recording ----------
  const rec = new Map(); // id -> {pid, file, startedAt, mode}

  async function recordingStart({ display, durationSec, region } = {}) {
    const dir = recordingsDir();
    fs.mkdirSync(dir, { recursive: true });
    const id = crypto.randomBytes(4).toString("hex");
    const file = path.join(dir, `rec-${id}.mov`);
    const args = ["-v", "-x"];
    const disp = display ?? state.activeDisplay;
    if (disp && disp !== "all") args.push("-D", String(disp));
    if (durationSec) args.push("-V", String(Math.max(1, Math.round(durationSec))));
    if (region) args.unshift("-R", region.join(","));
    args.push(file);
    const child = spawn("screencapture", args, { stdio: "ignore", detached: true });
    child.unref();
    const startedAt = new Date().toISOString();
    rec.set(id, { pid: child.pid, file, startedAt, mode: "screencapture", display: disp ?? 1 });
    if (durationSec) {
      const timer = setTimeout(() => rec.delete(id), (Math.round(durationSec) + 10) * 1000);
      timer.unref?.();
    }
    await new Promise((res) => setTimeout(res, 400));
    try { process.kill(child.pid, 0); } catch {
      rec.delete(id);
      throw new ExecError("screencapture -v exited immediately — screen recording permission (Screen & System Recording) is likely missing for the host terminal");
    }
    return { id, pid: child.pid, file, display: disp ?? 1, durationSec: durationSec ?? null, region: region ?? null, fps: "device-default", startedAt };
  }

  async function recordingStop({ id }) {
    const r = rec.get(id);
    if (!r) throw new ExecError(`unknown or already-finished recording "${id}" (recording_status/recording_list shows current state)`);
    let mp4 = null;
    try { process.kill(r.pid, "SIGINT"); } catch {}
    await new Promise((res) => setTimeout(res, 1500));
    const ffmpeg = await import("../exec.mjs").then((m) => m.have("ffmpeg"));
    if (ffmpeg && fs.existsSync(r.file)) {
      const out = r.file.replace(/\.mov$/, ".mp4");
      const rr = await runL("ffmpeg", ["-y", "-loglevel", "error", "-i", r.file, "-c", "copy", out], { timeoutMs: 120_000 });
      if (rr.code === 0) mp4 = out;
    }
    const size = fs.existsSync(r.file) ? fs.statSync(r.file).size : 0;
    rec.delete(id);
    return { id, file: r.file, mp4, bytes: size, startedAt: r.startedAt, stoppedAt: new Date().toISOString() };
  }

  async function recordingStatus({ id }) {
    const r = rec.get(id);
    if (!r) return { id, running: false };
    let alive = true;
    try { process.kill(r.pid, 0); } catch { alive = false; }
    return { id, running: alive, pid: r.pid, file: r.file, bytes: fs.existsSync(r.file) ? fs.statSync(r.file).size : 0, startedAt: r.startedAt };
  }

  async function recordingList() {
    const dir = recordingsDir();
    const out = [];
    for (const f of fs.existsSync(dir) ? fs.readdirSync(dir) : []) {
      const full = path.join(dir, f);
      const st = fs.statSync(full);
      if (st.isFile() && /\.(mov|mp4|png)$/i.test(f)) out.push({ file: full, bytes: st.size, modifiedAt: st.mtime.toISOString() });
    }
    out.sort((a, b) => b.modifiedAt.localeCompare(a.modifiedAt));
    return { dir, recordings: out.slice(0, 50), running: [...rec.keys()] };
  }

  // ---------- apps / windows ----------
  async function listApps() {
    return jxa(`${JXA_PRELUDE}
      var se = Application('System Events');
      var procs = se.applicationProcesses();
      var out = [];
      for (var i=0;i<procs.length;i++){
        var p = procs[i];
        var bg = g(function(){ return !!p.backgroundOnly(); });
        var name = g(function(){ return String(p.name()); });
        var wins = g(function(){ return p.windows(); });
        out.push({ name: name, pid: num(function(){ return p.unixId(); }),
          bundle_id: g(function(){ var b = p.bundleIdentifier(); return b? String(b): null; }),
          frontmost: g(function(){ return !!p.frontmost(); }),
          hidden: g(function(){ return !!p.hidden(); }),
          windowCount: wins ? wins.length : 0 });
      }
      return JSON.stringify({ apps: out });
    }`, {}, 25_000);
  }

  async function listWindows(appRef) {
    const p = await findProcess(appRef ?? {});
    if (!p.found) throw new ExecError("application not found — call list_apps for exact names/pids");
    return { app: { name: p.name, pid: p.pid, bundle_id: p.bundle_id }, windows: p.windows };
  }

  async function openApplication({ name, bundle_id: bid, pid, url: urlArg, activate = false } = {}) {
    if (!name && !bid) throw new ExecError("open_application needs name or bundle_id");
    const args = [];
    if (urlArg) args.push(urlArg);
    if (bid) args.unshift("-b", bid); else args.unshift("-a", name);
    if (activate) args.unshift("-F");
    const r = await runL("open", args, { timeoutMs: 25_000 });
    if (r.code !== 0) throw new ExecError(`open failed: ${r.stderr.trim().slice(0, 200)}`, r);
    await new Promise((res) => setTimeout(res, 600));
    const find = {};
    if (bid) find.bundle_id = bid; else if (pid) find.pid = pid; else find.name = String(name).replace(/\.app$/, "");
    const p = await findProcess(find).catch(() => null);
    return { launched: true, activate, url: urlArg ?? null, resolved: p?.found ? { name: p.name, pid: p.pid, bundle_id: p.bundle_id, frontmost: p.frontmost } : null };
  }

  // ---------- clipboard / cursor / waits ----------
  async function readClipboard() {
    const r = await runL("pbpaste", [], { timeoutMs: 5_000, maxBuffer: 4 * 1024 * 1024 });
    return { text: r.stdout, encoding: "utf8" };
  }
  async function writeClipboard({ text }) {
    const child = spawn("pbcopy", [], { stdio: ["pipe", "ignore", "ignore"] });
    child.stdin.end(String(text ?? ""));
    await new Promise((res, rej) => { child.on("close", res); child.on("error", rej); });
    return { written: String(text ?? "").length };
  }
  async function cursorPosition() {
    // Quartz (pyobjc) first — JXA cannot bridge CGEventGetLocation's CGPoint.
    const py = `from Quartz import CGEventCreate
l = CGEventCreate(None).location
print('{\"x\": %d, \"y\": %d}' % (l.x, l.y))`;
    const r = await runL("python3", ["-c", py], { timeoutMs: 8_000 });
    if (r.code === 0) {
      const j = tryJson(r.stdout.trim(), null);
      if (j && Number.isFinite(j.x)) return { x: j.x, y: j.y };
    }
    const cc = await runL("cliclick", ["p"], { timeoutMs: 8_000 });
    if (cc.code === 0) {
      const m = /(-?\d+)\s*,\s*(-?\d+)/.exec(cc.stdout.trim());
      if (m) return { x: Number(m[1]), y: Number(m[2]) };
    }
    throw new ExecError("cursor position needs python3 with pyobjc (Quartz) or cliclick on PATH");
  }

  // ---------- probe ----------
  async function probe() {
    const caps = { screenshot: true, recording: true, accessibility_tree: true, clipboard: true, displays: true };
    const perms = {};
    try { await jxa(`function run(argv){ return JSON.stringify({n: Application('System Events').applicationProcesses.length}); }`, {}, 8_000); perms.accessibility = "granted"; }
    catch (e) { perms.accessibility = "denied_or_unavailable"; caps.accessibility_tree = false; caps.raw_input = "unreliable"; }
    try {
      const t = os.tmpdir() + `/cu-probe-${crypto.randomBytes(3).toString("hex")}.png`;
      const r = await runL("screencapture", ["-x", "-R0,0,2,2", "-t", "png", t], { timeoutMs: 8_000 });
      perms.screen_capture = r.code === 0 ? "ok" : "failed";
      try { fs.rmSync(t, { force: true }); } catch {}
    } catch { perms.screen_capture = "failed"; }
    const hasRecording = fs.existsSync("/usr/sbin/screencapture");
    return { platform: "darwin", capabilities: caps, permissions: perms, note: "macOS does not expose Screen-Recording TCC state to CLI; a black/empty screenshot means Screen Recording permission is missing. Raw pointer/keyboard events go to whatever is frontmost at the target point — activate the app first for click-type actions." };
  }

  return {
    platform: "darwin",
    probe,
    list_displays: displayInfo,
    async switch_display({ index }) {
      const ds = await displayInfo();
      if (!ds.some((d) => d.index === index)) throw new ExecError(`no display ${index}; have [${ds.map((d) => d.index).join(", ")}]`);
      state.activeDisplay = index;
      return { activeDisplay: index };
    },
    list_apps: listApps,
    list_windows: listWindows,
    open_application: openApplication,
    get_app_state: async ({ app_ref, detail, depth }) => {
      const t = await walkTree(app_ref ?? {}, detail === "full" ? 12 : 8, detail === "full" ? 800 : 400);
      if (!t.found) throw new ExecError("application not found — call list_apps for exact names/pids");
      return t;
    },
    screenshot,
    zoom,
    left_click: ({ target }) => pointerClick("left", target.x, target.y, 1),
    double_click: ({ target }) => pointerClick("left", target.x, target.y, 2),
    triple_click: ({ target }) => pointerClick("left", target.x, target.y, 3),
    right_click: ({ target }) => pointerClick("right", target.x, target.y, 1),
    middle_click: ({ target }) => pointerClick("middle", target.x, target.y, 1),
    mouse_move: async ({ target }) => {
      assertInScreen(target.x, target.y);
      await postMouseEvent(5, target.x, target.y, 0, 0);
      return { action_sent: true, at: { x: target.x, y: target.y } };
    },
    left_mouse_down: async ({ target }) => { assertInScreen(target.x, target.y); await postMouseEvent(MOUSE.left.down, target.x, target.y, 0, 1); return { action_sent: true }; },
    left_mouse_up: async ({ target }) => {
      const loc = await cursorPosition();
      await postMouseEvent(MOUSE.left.up, loc.x, loc.y, 0, 1);
      return { action_sent: true };
    },
    left_click_drag: async ({ from_target: from, to }) => {
      assertInScreen(from.x, from.y); assertInScreen(to.x, to.y);
      await postMouseEvent(MOUSE.left.down, from.x, from.y, 0, 1);
      const steps = 12;
      for (let i = 1; i <= steps; i++) {
        await new Promise((r) => setTimeout(r, 24));
        await postMouseEvent(MOUSE.left.dragged, from.x + ((to.x - from.x) * i) / steps, from.y + ((to.y - from.y) * i) / steps, 0, 1);
      }
      await new Promise((r) => setTimeout(r, 60));
      await postMouseEvent(MOUSE.left.up, to.x, to.y, 0, 1);
      return { action_sent: true, from, to };
    },
    scroll: async ({ target, direction = "down", amount = 5 }) => {
      assertInScreen(target.x, target.y);
      await postMouseEvent(5, target.x, target.y, 0, 0);
      const dx = direction === "left" ? -amount : direction === "right" ? amount : 0;
      const dy = direction === "up" ? amount : direction === "down" ? -amount : 0;
      return cg(`var ev = $.CGEventCreateScrollWheelEvent($(), 1, 2, P.dy, P.dx);
        $.CGEventPost($.kCGHIDEventTap, ev);
        return JSON.stringify({ ok: true });`, { dx, dy }).then(() => ({ action_sent: true, direction, amount }));
    },
    type: async ({ text }) => {
      if (!text) return { action_sent: false, note: "empty text" };
      const r = await cg(`var ev = $.CGEventCreateKeyboardEvent($(), 0, true);
        $.CGEventKeyboardSetUnicodeString(ev, P.text.length, P.text);
        $.CGEventPost($.kCGHIDEventTap, ev);
        var ev2 = $.CGEventCreateKeyboardEvent($(), 0, false);
        $.CGEventKeyboardSetUnicodeString(ev2, P.text.length, P.text);
        $.CGEventPost($.kCGHIDEventTap, ev2);
        return JSON.stringify({ ok: true, chars: P.text.length });`, { text }, 15_000);
      return { action_sent: true, chars: text.length, strategy: "unicode-events" };
    },
    key: async ({ text, repeat = 1 }) => {
      const { flags, code, key } = parseChord(text);
      for (let i = 0; i < Math.max(1, Math.min(100, repeat)); i++) {
        await keyEvent(code, flags, true);
        await keyEvent(code, flags, false);
        if (i < repeat - 1) await new Promise((r) => setTimeout(r, 30));
      }
      return { action_sent: true, key, code, repeat: Math.max(1, Math.min(100, repeat)) };
    },
    hold_key: async ({ text, duration }) => {
      const { flags, code, key } = parseChord(text);
      const d = Math.max(0.05, Math.min(30, Number(duration) || 1));
      await keyEvent(code, flags, true);
      await new Promise((r) => setTimeout(r, d * 1000));
      await keyEvent(code, flags, false);
      return { action_sent: true, key, heldSec: d };
    },
    set_value: async ({ target, value }) => {
      const el = await elementAction(target.app_ref, target.windowIndex, target.path, { kind: "set_value", value });
      if (!el.ok) throw new ExecError(`set_value failed: ${el.code}`);
      return { action_sent: true, strategy: "a11y", element: el.element };
    },
    select_text: async ({ target, text_range }) => {
      const el = await elementAction(target.app_ref, target.windowIndex, target.path, { kind: "select_text", range: text_range });
      if (!el.ok) throw new ExecError(`select_text failed: ${el.code}`);
      return { action_sent: true, strategy: "a11y", element: el.element };
    },
    perform_action: async ({ target, action }) => {
      const el = await elementAction(target.app_ref, target.windowIndex, target.path, { kind: "action", action });
      if (!el.ok) throw new ExecError(`perform_action failed: ${el.code} — check the element's actions list from get_app_state`);
      return { action_sent: el.sent, strategy: "a11y", action, element: el.element };
    },
    read_clipboard: readClipboard,
    write_clipboard: writeClipboard,
    cursor_position: cursorPosition,
    recordingStart,
    recordingStop,
    recordingStatus,
    recordingList,
  };
}

export default { create };
