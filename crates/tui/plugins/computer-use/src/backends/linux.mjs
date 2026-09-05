// Linux backend — X11 first (xdotool/wmctrl/scrot/xclip), Wayland where the
// right tools exist (grim/wtype/ydotool/wf-recorder/wl-clipboard). The
// accessibility tree comes from AT-SPI via python3+pyatspi when installed.
// Everything probes at call time and fails closed with the missing tool named.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawn } from "node:child_process";
import { run, runOk, ExecError, tryJson, have } from "../exec.mjs";

const XKEYS = {
  return: "Return", enter: "Return", tab: "Tab", escape: "Escape", esc: "Escape",
  space: "space", backspace: "BackSpace", delete: "Delete", home: "Home", end: "End",
  pageup: "Page_Up", pagedown: "Page_Down", left: "Left", right: "Right", up: "Up",
  down: "Down", capslock: "Caps_Lock", menu: "Menu", print: "Print",
};

function spawnDetached(cmd, args, stdinText = "", quiet = true) {
  const child = spawn(cmd, args, {
    stdio: stdinText ? ["pipe", "ignore", quiet ? "ignore" : "pipe"] : ["ignore", "ignore", quiet ? "ignore" : "pipe"],
    detached: true,
  });
  if (stdinText) child.stdin.end(stdinText);
  child.unref();
  return child;
}

export function create({ exec }) {
  const tools = {};
  let session = null; // "x11" | "wayland"
  let probed = false;
  let lastRaster = null;
  let recording = null; // {id, pid, file, startedAt, mode}

  async function probeSession() {
    if (probed) return session;
    probed = true;
    const wayland = !!(process.env.WAYLAND_DISPLAY || process.env.XDG_SESSION_TYPE === "wayland");
    const x11 = !!(process.env.DISPLAY || process.env.XDG_SESSION_TYPE === "x11");
    session = wayland && !x11 ? "wayland" : x11 ? "x11" : null;
    for (const t of ["xdotool", "wmctrl", "scrot", "import", "grim", "slurp", "wtype", "ydotool", "wf-recorder", "ffmpeg", "xclip", "xsel", "wl-copy", "wl-paste", "python3", "xrandr", "swaymsg", "hyprctl"]) {
      tools[t] = await have(t);
    }
    tools.pyatspi = tools.python3 && (await run("python3", ["-c", "import pyatspi"], { timeoutMs: 10_000 })).code === 0;
    return session;
  }

  function need(tool, purpose) {
    if (!tools[tool]) throw new ExecError(`linux backend needs "${tool}" for ${purpose} — install it and retry`);
  }

  async function shotTool() {
    if (session === "wayland") { need("grim", "screenshots on Wayland"); return { cmd: "grim", base: [] }; }
    if (session === "x11") {
      if (tools.scrot) return { cmd: "scrot", base: ["-z"] };
      need("import", "screenshots on X11 (imagemagick)");
      return { cmd: "import", base: ["-window", "root"] };
    }
    throw new ExecError("no X11 ($DISPLAY) or Wayland ($WAYLAND_DISPLAY) session visible to this process");
  }

  function recordingsDir() {
    return process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
  }

  async function xdotool(args, opts = {}) {
    need("xdotool", "input on X11");
    const r = await run("xdotool", args, opts);
    if (r.code !== 0) throw new ExecError(`xdotool ${args[0]} exited ${r.code}: ${r.stderr.trim().slice(0, 200)}`, r);
    return r.stdout.trim();
  }

  async function ydotool(args, opts = {}) {
    need("ydotool", "input on Wayland (ydotool needs its daemon running: sudo ydotoold)");
    const r = await run("ydotool", args, opts);
    if (r.code !== 0) throw new ExecError(`ydotool exited ${r.code}: ${r.stderr.trim().slice(0, 200)}`, r);
    return r.stdout.trim();
  }

  function xdotoolKey(text) {
    return String(text).split("+").map((p) => {
      const k = p.trim().toLowerCase();
      if (XKEYS[k]) return XKEYS[k];
      if (/^f\d{1,2}$/.test(k)) return k.toUpperCase();
      return p.trim(); // pass through names already in xdotool form
    }).join("+");
  }

  function assertNum(v, name) {
    const n = Number(v);
    if (!Number.isFinite(n)) throw new ExecError(`${name} must be a finite number`);
    return n;
  }

  // ---------- AT-SPI tree ----------
  const PYATSPI_WALK = `import json, sys, pyatspi
app_name = sys.argv[1] if len(sys.argv) > 1 else None
depth_max = int(sys.argv[2]) if len(sys.argv) > 2 else 8
max_el = int(sys.argv[3]) if len(sys.argv) > 3 else 400
desktop = pyatspi.Registry.getDesktop(0)
root = None
if app_name:
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app and app_name.lower() in (app.name or "").lower():
            root = app
            break
else:
    for i in range(desktop.childCount):
        a = desktop.getChildAtIndex(i)
        if a and a.childCount:
            root = a
            break
if root is None:
    print(json.dumps({"found": False}))
    sys.exit(0)
els = []
truncated = False
def info(e, path):
    ext = None
    try: ext = e.getExtents(pyatspi.DESKTOP_COORDS)
    except Exception: pass
    txt = None
    try:
        q = e.queryText()
        n = min(120, q.characterCount)
        if n > 0: txt = q.getText(0, n)
    except Exception: pass
    acts = []
    try:
        a = e.queryAction()
        acts = [a.getName(i) for i in range(a.nActions)]
    except Exception: pass
    els.append({"index": len(els), "path": path, "role": e.getRoleName() if e.getRoleName() else None,
                "label": e.name or None, "value": txt,
                "position": {"x": ext.x, "y": ext.y} if ext else None,
                "size": {"w": ext.width, "h": ext.height} if ext else None,
                "actions": acts})
def walk(e, path, d):
    global truncated
    if len(els) >= max_el or d > depth_max:
        truncated = True
        return
    try: info(e, path)
    except Exception: return
    for i in range(e.childCount):
        try: c = e.getChildAtIndex(i)
        except Exception: continue
        if c: walk(c, path + [i], d + 1)
walk(root, [], 0)
print(json.dumps({"found": True, "name": root.name, "elements": els, "truncated": truncated}))`;

  async function atspiResolve(pathArr, pythonBody, extraArg = null) {
    need("python3", "semantic element actions (AT-SPI)");
    const script = `import json, sys, pyatspi
desktop = pyatspi.Registry.getDesktop(0)
target_path = json.loads(sys.argv[1])
extra = sys.argv[2] if len(sys.argv) > 2 else None
node = None
for i in range(desktop.childCount):
    a = desktop.getChildAtIndex(i)
    if a and a.childCount:
        node = a
        break
if node is None:
    print(json.dumps({"ok": False, "code": "app_not_found"}))
    sys.exit(0)
found = None
stack = [(node, [])]
while stack:
    n, p = stack.pop(0)
    if p == target_path:
        found = n
        break
    if len(p) > 12: continue
    try:
        for i in range(n.childCount):
            c = n.getChildAtIndex(i)
            if c: stack.append((c, p + [i]))
    except Exception: pass
if found is None:
    print(json.dumps({"ok": False, "code": "element_stale"}))
    sys.exit(0)
try:
${pythonBody}
except Exception as e:
    print(json.dumps({"ok": False, "code": str(e)}))`;
    const argv = ["-c", script, JSON.stringify(pathArr ?? [])];
    if (extraArg != null) argv.push(String(extraArg));
    const r = await run("python3", argv, { timeoutMs: 30_000 });
    const out = tryJson((r.stdout.trim().split("\n").pop() ?? ""), null);
    if (!out) throw new ExecError(`AT-SPI action failed: ${(r.stderr || r.stdout).slice(0, 250)}`, r);
    return out;
  }

  // ---------- input helpers ----------
  function clickButton(button, clicks) {
    if (session === "x11") {
      const args = ["click"];
      if (clicks > 1) args.push("--repeat", String(clicks), "--delay", "80");
      args.push(String(button));
      return xdotool(args);
    }
    // ydotool click mask: down|up|count nibble (0xC0 = left click once, +1 per extra click;
    // 0x04 bit selects right button, 0x02 middle).
    const count = Math.max(1, Math.min(3, clicks));
    const code = button === 3 ? 0xc0 + count + 0x04 : button === 2 ? 0xc0 + count + 0x02 : 0xc0 + count - 1;
    return ydotool(["click", "0x" + code.toString(16)]);
  }

  return {
    platform: "linux",
    probe: async () => {
      const s = await probeSession();
      const caps = {
        screenshot: !!((session === "wayland" && tools.grim) || (session === "x11" && (tools.scrot || tools.import))),
        clipboard: !!(tools.xclip || tools.xsel || (tools["wl-copy"] && tools["wl-paste"])),
        recording: !!(tools.ffmpeg || tools["wf-recorder"]),
        accessibility_tree: tools.pyatspi,
      };
      const missing = [];
      if (session === "x11" && !tools.xdotool) missing.push("xdotool (input)");
      if (session === "wayland" && !tools.ydotool) missing.push("ydotool+ydotoold (mouse input)");
      if (session === "wayland" && !tools.grim) missing.push("grim (screenshots)");
      if (session === "x11" && !tools.scrot && !tools.import) missing.push("scrot or imagemagick (screenshots)");
      if (!caps.recording) missing.push("ffmpeg (X11) or wf-recorder (Wayland)");
      if (!tools.pyatspi) missing.push("python3-pyatspi (accessibility tree)");
      return { platform: "linux", session: s, capabilities: caps, missing, note: "Every capability probes at call time and fails closed naming the missing tool." };
    },
    list_displays: async () => {
      await probeSession();
      if (session === "x11" && tools.xrandr) {
        const r = await runOk("xrandr", ["--query"], { timeoutMs: 15_000 });
        const displays = [];
        let i = 1;
        for (const m of r.stdout.matchAll(/^(\S+) connected (?:primary )?(\d+)x(\d+)\+(\d+)\+(\d+)/gm)) {
          displays.push({ index: i++, name: m[1], points: { x: Number(m[4]), y: Number(m[5]), w: Number(m[2]), h: Number(m[3]) }, pixels: { w: Number(m[2]), h: Number(m[3]) }, scale: 1, main: /primary/.test(m[0]) || i === 1 });
        }
        if (displays.length) return displays;
      }
      if (session === "wayland" && tools.swaymsg) {
        const r = await run("swaymsg", ["-t", "get_outputs", "-r"], { timeoutMs: 15_000 });
        const outs = tryJson(r.stdout, []);
        if (Array.isArray(outs) && outs.length) {
          return outs.map((o, i) => ({ index: i + 1, name: o.name, points: { x: o.rect?.x, y: o.rect?.y, w: o.rect?.width, h: o.rect?.height }, pixels: { w: o.current_mode?.width, h: o.current_mode?.height }, scale: o.scale ?? 1, main: i === 0 }));
        }
      }
      if (session === "wayland" && tools.hyprctl) {
        const r = await run("hyprctl", ["-j", "monitors"], { timeoutMs: 15_000 });
        const ms = tryJson(r.stdout, []);
        if (Array.isArray(ms) && ms.length) {
          return ms.map((o, i) => ({ index: i + 1, name: o.name, points: { x: o.x, y: o.y, w: o.width, h: o.height }, pixels: { w: o.width, h: o.height }, scale: o.scale ?? 1, main: !!o.main || i === 0 }));
        }
      }
      throw new ExecError("display enumeration needs xrandr (X11) or swaymsg/hyprctl (Wayland) — install one and retry");
    },
    switch_display: async ({ index }) => ({ activeDisplay: index ?? 1, note: "linux screenshots grab the compositor's virtual screen; per-display selection applies only where the shot tool supports it" }),
    list_apps: async () => {
      await probeSession();
      if (session === "x11" && tools.wmctrl) {
        const r = await runOk("wmctrl", ["-lx"], { timeoutMs: 15_000 });
        const seen = new Map();
        for (const line of r.stdout.split("\n")) {
          const parts = line.split(/\s+/);
          const wmClass = parts[2];
          if (wmClass) seen.set(wmClass, { name: wmClass.split(".")[0], wm_class: wmClass });
        }
        return { apps: [...seen.values()] };
      }
      if (session === "wayland" && (tools.swaymsg || tools.hyprctl)) {
        const w = await this.list_windows();
        const seen = new Map();
        for (const win of w.windows) {
          const cls = win.wm_class || win.app_id;
          if (cls) seen.set(cls, { name: cls, wm_class: cls });
        }
        return { apps: [...seen.values()] };
      }
      throw new ExecError("list_apps needs wmctrl (X11) or swaymsg/hyprctl (Wayland)");
    },
    list_windows: async () => {
      await probeSession();
      if (session === "x11" && tools.wmctrl) {
        const r = await runOk("wmctrl", ["-lGx"], { timeoutMs: 15_000 });
        const windows = [];
        for (const line of r.stdout.split("\n")) {
          const m = /^(\S+)\s+(-?\d+)\s+(-?\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+)\s+(.*)$/.exec(line);
          if (m) windows.push({ id: m[1], desktop: m[2], position: { x: Number(m[3]), y: Number(m[4]) }, size: { w: Number(m[5]), h: Number(m[6]) }, wm_class: m[7], title: m[8] });
        }
        return { windows };
      }
      if (session === "wayland" && tools.swaymsg) {
        const r = await run("swaymsg", ["-t", "get_tree", "-r"], { timeoutMs: 15_000 });
        const windows = [];
        const walk = (n) => {
          if (n.type === "con" && n.name) windows.push({ id: String(n.id), title: n.name, wm_class: n.app_id ?? null, position: { x: n.rect?.x, y: n.rect?.y }, size: { w: n.rect?.width, h: n.rect?.height }, focused: !!n.focused });
          (n.nodes ?? []).forEach(walk);
          (n.floating_nodes ?? []).forEach(walk);
        };
        walk(tryJson(r.stdout, {}));
        return { windows };
      }
      if (session === "wayland" && tools.hyprctl) {
        const r = await run("hyprctl", ["-j", "clients"], { timeoutMs: 15_000 });
        const clients = tryJson(r.stdout, []);
        return { windows: clients.map((c) => ({ id: String(c.address), title: c.title, wm_class: c.class, position: { x: c.at?.[0], y: c.at?.[1] }, size: { w: c.size?.[0], h: c.size?.[1] }, focused: !!c.focused })) };
      }
      throw new ExecError("list_windows needs wmctrl (X11), swaymsg (sway) or hyprctl (hyprland)");
    },
    open_application: async ({ name, bundle_id: bid, url: urlArg } = {}) => {
      const target = name ?? bid;
      if (!target || !/^[A-Za-z0-9][A-Za-z0-9 ._-]*$/.test(target)) throw new ExecError("open_application needs a plain executable/desktop name");
      spawnDetached(target, urlArg ? [urlArg] : [], "", true);
      await new Promise((r) => setTimeout(r, 500));
      return { launched: true, name: target, url: urlArg ?? null };
    },
    get_app_state: async ({ app_ref } = {}) => {
      const t = await run("python3", ["-c", PYATSPI_WALK, app_ref?.name ?? "", "10", "500"], { timeoutMs: 45_000 }).then((r) =>
        tryJson((r.stdout.trim().split("\n").pop() ?? ""), null));
      if (!t) throw new ExecError("AT-SPI walk failed — is python3-pyatspi installed and the desktop running an accessibility bus (AT_SPI_BUS)?");
      if (!t.found) throw new ExecError("application not found in the AT-SPI tree — pass app_ref.name from list_apps");
      return t;
    },
    screenshot: async ({ display, region, path: outPath } = {}) => {
      await probeSession();
      const { cmd, base } = await shotTool();
      const dir = recordingsDir();
      fs.mkdirSync(dir, { recursive: true });
      const file = outPath || path.join(dir, `shot-${new Date().toISOString().replace(/[:.]/g, "-")}-${crypto.randomBytes(3).toString("hex")}.png`);
      let args = [...base];
      if (cmd === "grim") {
        if (region) args.push("-g", `${Math.round(region[0])},${Math.round(region[1])} ${Math.round(region[2])}x${Math.round(region[3])}`);
        args.push(file);
      } else if (cmd === "scrot") {
        if (region) args.push("-a", `${Math.round(region[0])},${Math.round(region[1])},${Math.round(region[2])},${Math.round(region[3])}`);
        args.push(file);
      } else {
        if (region) args.push("-crop", `${Math.round(region[2])}x${Math.round(region[3])}+${Math.round(region[0])}+${Math.round(region[1])}`);
        args.push(file);
      }
      const r = await run(cmd, args, { timeoutMs: 20_000 });
      if (r.code !== 0) throw new ExecError(`${cmd} exited ${r.code}: ${r.stderr.trim().slice(0, 300)}`, r);
      lastRaster = { file, bytes: fs.statSync(file).size, capturedAt: new Date().toISOString() };
      return { ...lastRaster };
    },
    zoom: async ({ source, region, path: outPath }) => {
      need("ffmpeg", "zoom/crop");
      const src = source ?? lastRaster?.file;
      if (!src) throw new ExecError("no screenshot taken yet on this computer — call screenshot first");
      const out = outPath || path.join(recordingsDir(), `zoom-${crypto.randomBytes(4).toString("hex")}.png`);
      await runOk("ffmpeg", ["-y", "-loglevel", "error", "-i", src, "-vf", `crop=${Math.round(region[2])}:${Math.round(region[3])}:${Math.round(region[0])}:${Math.round(region[1])}`, out], { timeoutMs: 20_000 });
      return { file: out, bytes: fs.statSync(out).size, region, source: src };
    },
    left_click: ({ target }) => { assertNum(target.x, "x"); assertNum(target.y, "y"); return inputChain(target.x, target.y, () => clickButton(1, 1)); },
    double_click: ({ target }) => inputChain(target.x, target.y, () => clickButton(1, 2)),
    triple_click: ({ target }) => inputChain(target.x, target.y, () => clickButton(1, 3)),
    right_click: ({ target }) => inputChain(target.x, target.y, () => clickButton(3, 1)),
    middle_click: ({ target }) => inputChain(target.x, target.y, () => clickButton(2, 1)),
    mouse_move: ({ target }) => inputMove(target.x, target.y),
    left_click_drag: async ({ from_target: from, to }) => {
      await inputMove(from.x, from.y);
      if (session === "x11") await xdotool(["mousedown", "1"]);
      else await ydotool(["click", "0x40"]);
      for (let i = 1; i <= 10; i++) {
        await new Promise((r) => setTimeout(r, 20));
        await inputMove(from.x + ((to.x - from.x) * i) / 10, from.y + ((to.y - from.y) * i) / 10);
      }
      if (session === "x11") await xdotool(["mouseup", "1"]);
      else await ydotool(["click", "0x80"]);
      return { action_sent: true, from, to };
    },
    left_mouse_down: ({ target }) => session === "x11" ? xdotool(["mousedown", "1"]).then(() => ({ action_sent: true })) : ydotool(["click", "0x40"]).then(() => ({ action_sent: true })),
    left_mouse_up: () => session === "x11" ? xdotool(["mouseup", "1"]).then(() => ({ action_sent: true })) : ydotool(["click", "0x80"]).then(() => ({ action_sent: true })),
    scroll: async ({ target, direction = "down", amount = 3 }) => {
      await inputMove(target.x, target.y);
      if (session === "x11") {
        const buttons = { down: 5, up: 4, right: 7, left: 6 };
        await xdotool(["click", "--repeat", String(Math.max(1, Math.min(30, amount))), "--delay", "60", String(buttons[direction] ?? 5)]);
        return { action_sent: true, direction, amount };
      }
      // Wayland: synthesize wheel via ydotool is not wired in this build — honest refusal.
      throw new ExecError('scroll on Wayland is not available in this build; use swipe-style drags or run an X11/XWayland window. (Roadmap: ydotool wheel events.)');
    },
    type: async ({ text }) => {
      if (!text) return { action_sent: false, note: "empty text" };
      await probeSession();
      if (session === "x11") {
        await xdotool(["type", "--delay", "12", "--", String(text)]);
        return { action_sent: true, chars: text.length };
      }
      need("wtype", "typing on Wayland");
      const r = await run("wtype", ["--", String(text)], { timeoutMs: 15_000 });
      if (r.code !== 0) throw new ExecError(`wtype failed: ${r.stderr.slice(0, 200)}`, r);
      return { action_sent: true, chars: text.length };
    },
    key: async ({ text, repeat = 1 }) => {
      await probeSession();
      const k = xdotoolKey(text);
      if (session === "x11") {
        await xdotool(["key", "--repeat", String(Math.max(1, Math.min(100, repeat))), "--delay", "60", k]);
        return { action_sent: true, key: k };
      }
      need("wtype", "key presses on Wayland");
      await run("wtype", ["-P", k]);
      await run("wtype", ["-R", k]);
      return { action_sent: true, key: k };
    },
    hold_key: async ({ text, duration }) => {
      await probeSession();
      const k = xdotoolKey(text);
      const d = Math.max(0.05, Math.min(30, Number(duration) || 1));
      if (session === "x11") {
        await xdotool(["keydown", k]);
        await new Promise((r) => setTimeout(r, d * 1000));
        await xdotool(["keyup", k]);
        return { action_sent: true, key: k, heldSec: d };
      }
      need("ydotool", "key hold on Wayland");
      await ydotool(["key", `${k}:1`]);
      await new Promise((r) => setTimeout(r, d * 1000));
      await ydotool(["key", `${k}:0`]);
      return { action_sent: true, key: k, heldSec: d };
    },
    set_value: async ({ target, value }) => {
      const out = await atspiResolve(
        target.path,
        `    v = found.queryValue()
    v.currentValue = float(extra)`,
        value,
      ).catch(async (e) => {
        // Fall back to the Text interface for text-bearing widgets.
        const out2 = await atspiResolve(
          target.path,
          `    t = found.queryText()
    t.setTextContents(extra)`,
          String(value),
        );
        if (!out2.ok) throw e;
        return out2;
      });
      if (!out.ok) throw new ExecError(`set_value failed: ${out.code}`);
      return { action_sent: true, strategy: "a11y" };
    },
    select_text: async () => { throw new ExecError("select_text is not implemented on the linux backend — fail-closed"); },
    perform_action: async ({ target, action }) => {
      const body = `    a = found.queryAction()
    names = [a.getName(i) for i in range(a.nActions)]
    want = (extra or "click").lower()
    match = next((n for n in names if n.lower() == want), None)
    if match is None and want == "click":
        match = next((n for n in names if n.lower() in ("click", "press", "activate")), None)
    if match is None:
        print(json.dumps({"ok": False, "code": "action_not_found: " + ",".join(names)}))
    else:
        a.doAction(names.index(match))
        print(json.dumps({"ok": True, "sent": True}))`;
      const out = await atspiResolve(target.path, body, String(action));
      if (!out.ok) throw new ExecError(`perform_action failed: ${out.code}`);
      return { action_sent: true, strategy: "a11y", action };
    },
    read_clipboard: async () => {
      await probeSession();
      const cmd = session === "x11"
        ? (tools.xclip ? ["xclip", "-selection", "clipboard", "-o"] : ["xsel", "--clipboard", "--output"])
        : ["wl-paste"];
      need(cmd[0], "clipboard read");
      const r = await run(cmd[0], cmd.slice(1), { timeoutMs: 10_000 });
      if (r.code !== 0) throw new ExecError("clipboard read failed", r);
      return { text: r.stdout, encoding: "utf8" };
    },
    write_clipboard: async ({ text }) => {
      await probeSession();
      const cmd = session === "x11"
        ? (tools.xclip ? ["xclip", "-selection", "clipboard"] : ["xsel", "--clipboard", "--input"])
        : ["wl-copy"];
      need(cmd[0], "clipboard write");
      spawnDetached(cmd[0], cmd.slice(1), String(text ?? ""), true);
      return { written: String(text ?? "").length };
    },
    cursor_position: async () => {
      await probeSession();
      if (session === "x11") {
        const out = await xdotool(["getmouselocation"]);
        const m = /(\d+)\s+(\d+)/.exec(out);
        return { x: Number(m[1]), y: Number(m[2]) };
      }
      throw new ExecError("cursor position needs an X11 session in this build");
    },
    recordingStart: async ({ fps = 15, region } = {}) => {
      await probeSession();
      const dir = recordingsDir();
      fs.mkdirSync(dir, { recursive: true });
      const id = crypto.randomBytes(4).toString("hex");
      const file = path.join(dir, `rec-${id}.${session === "wayland" ? "mkv" : "mp4"}`);
      if (session === "x11") {
        need("ffmpeg", "recording on X11");
        const dpy = process.env.DISPLAY || ":0";
        const args = ["-y", "-loglevel", "error", "-f", "x11grab", "-framerate", String(fps)];
        if (region) args.push("-video_size", `${Math.round(region[2])}x${Math.round(region[3])}`);
        args.push("-i", `${dpy}${region ? `+${Math.round(region[0])},${Math.round(region[1])}` : ""}`, "-c:v", "libx264", "-pix_fmt", "yuv420p", file);
        const child = spawnDetached("ffmpeg", args);
        await new Promise((r) => setTimeout(r, 700));
        try { process.kill(child.pid, 0); } catch { throw new ExecError("ffmpeg x11grab exited immediately — check DISPLAY, XAUTHORITY and screen permissions"); }
        recording = { id, pid: child.pid, file, startedAt: new Date().toISOString(), mode: "x11grab" };
        return { id, pid: child.pid, file, mode: "x11grab", fps };
      }
      need("wf-recorder", "recording on Wayland");
      const args = ["-r", String(fps), "-f", file];
      if (process.env.CU_WAYLAND_OUTPUT) args.unshift("-o", process.env.CU_WAYLAND_OUTPUT);
      const child = spawnDetached("wf-recorder", args);
      await new Promise((r) => setTimeout(r, 700));
      try { process.kill(child.pid, 0); } catch { throw new ExecError("wf-recorder exited immediately — check compositor support (wlroots)"); }
      recording = { id, pid: child.pid, file, startedAt: new Date().toISOString(), mode: "wf-recorder" };
      return { id, pid: child.pid, file, mode: "wf-recorder", fps };
    },
    recordingStop: async ({ id }) => {
      if (!recording || recording.id !== id) throw new ExecError(`unknown recording "${id}"`);
      process.kill(recording.pid, "SIGINT");
      await new Promise((r) => setTimeout(r, 1500));
      const bytes = fs.existsSync(recording.file) ? fs.statSync(recording.file).size : 0;
      const out = { id, file: recording.file, bytes, mode: recording.mode, startedAt: recording.startedAt, stoppedAt: new Date().toISOString() };
      recording = null;
      return out;
    },
    recordingStatus: ({ id }) => {
      if (!recording || recording.id !== id) return { id, running: false };
      let alive = true;
      try { process.kill(recording.pid, 0); } catch { alive = false; }
      return { id, running: alive, file: recording.file, bytes: fs.existsSync(recording.file) ? fs.statSync(recording.file).size : 0, mode: recording.mode };
    },
    recordingList: async () => {
      const dir = recordingsDir();
      const out = fs.existsSync(dir)
        ? fs.readdirSync(dir).filter((f) => /\.(mp4|mkv|png)$/i.test(f)).map((f) => {
            const st = fs.statSync(path.join(dir, f));
            return { file: path.join(dir, f), bytes: st.size, modifiedAt: st.mtime.toISOString() };
          }).sort((a, b) => b.modifiedAt.localeCompare(a.modifiedAt)).slice(0, 50)
        : [];
      return { dir, recordings: out, running: recording ? [recording.id] : [] };
    },
  };

  async function inputChain(x, y, act) {
    await inputMove(x, y);
    await act();
    return { action_sent: true, at: { x: Number(x), y: Number(y) } };
  }

  async function inputMove(x, y) {
    const nx = Math.round(assertNum(x, "x"));
    const ny = Math.round(assertNum(y, "y"));
    if (session === "x11") await xdotool(["mousemove", "--sync", String(nx), String(ny)]);
    else await ydotool(["moveto", String(nx), String(ny)]);
  }
}

export default { create };
