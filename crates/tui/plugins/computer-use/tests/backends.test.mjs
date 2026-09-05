// Backend tests that can run on any host: harmony logic via a mocked hdc
// exec, linux fail-closed probing, and module-shape checks for win32.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseBounds, flatten } from "../src/backends/harmonyos.mjs";

function fakeJpeg(w, h) {
  // Minimal JPEG with an SOF0 marker carrying the dimensions.
  return Buffer.from([
    0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 0x08, (h >> 8) & 0xff, h & 0xff,
    (w >> 8) & 0xff, w & 0xff, 0x01, 0x01, 0x11, 0x00, 0xff, 0xd9,
  ]);
}

function harmonyFixtureExec(t) {
  const layout = {
    attributes: { bundleName: "com.example.app", type: "FrameNode" },
    children: [
      {
        attributes: { type: "Button", text: "OK", id: "ok_btn", bounds: "[100,200][300,260]" },
        children: [],
      },
      {
        attributes: { type: "Text", text: "Hello", bounds: "[0,0][100,50]" },
        children: [{ attributes: { type: "Text", text: "nested", bounds: "[10,10][90,40]" }, children: [] }],
      },
    ],
  };
  const calls = [];
  const exec = {
    targetArgs: [],
    run: async () => ({ code: 0, stdout: "", stderr: "" }),
    runOk: async () => ({ code: 0, stdout: "", stderr: "" }),
    shell: async (args) => { calls.push({ kind: "shell", args }); return { code: 0, stdout: "", stderr: "" }; },
    async pullFile(remote, local) {
      calls.push({ kind: "pull", remote, local });
      if (remote.includes("layout")) fs.writeFileSync(local, Buffer.from(JSON.stringify(layout)));
      else fs.writeFileSync(local, fakeJpeg(168, 120));
      return local;
    },
    async readFile(remote) {
      if (remote.includes("layout")) return Buffer.from(JSON.stringify(layout));
      return fakeJpeg(168, 120);
    },
  };
  return { exec, calls };
}

test("harmony: parseBounds handles uitest bounds strings", () => {
  assert.deepEqual(parseBounds("[100,200][300,260]"), { x: 100, y: 200, w: 200, h: 60, cx: 200, cy: 230 });
  assert.equal(parseBounds("garbage"), null);
});

test("harmony: flatten produces indexed elements with paths and geometry", () => {
  const tree = { attributes: { type: "root" }, children: [{ attributes: { type: "Button", text: "OK", bounds: "[0,0][10,10]" } }] };
  const els = flatten(tree);
  assert.equal(els[0].role, "root");
  assert.equal(els[0].path.length, 0);
  assert.equal(els[1].label, "OK");
  assert.deepEqual(els[1].path, [0]);
  assert.equal(els[1].bounds.cx, 5);
});

test("harmony: get_app_state flattens dumpLayout with indices and actions", async () => {
  const { exec } = harmonyFixtureExec();
  const mod = await import("../src/backends/harmonyos.mjs");
  const b = mod.create({ exec });
  const st = await b.get_app_state({});
  assert.equal(st.bundle_id, "com.example.app");
  assert.ok(st.elements.length >= 4);
  const ok = st.elements.find((e) => e.label === "OK");
  assert.ok(ok, "OK button flattened");
  assert.deepEqual(ok.bounds, { x: 100, y: 200, w: 200, h: 60, cx: 200, cy: 230 });
  assert.ok(ok.actions.includes("click"));
});

test("harmony: screenshot pulls the file and reports panel dimensions", async () => {
  const { exec } = harmonyFixtureExec();
  const mod = await import("../src/backends/harmonyos.mjs");
  const b = mod.create({ exec });
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cu-hm-test-"));
  const shot = await b.screenshot({ path: path.join(dir, "shot.jpeg") });
  assert.equal(shot.pixels.w, 168);
  assert.equal(shot.pixels.h, 120);
  assert.ok(fs.existsSync(shot.file));
  fs.rmSync(dir, { recursive: true, force: true });
});

test("harmony: click routes through uitest uiInput with validated args", async () => {
  const { exec, calls } = harmonyFixtureExec();
  const mod = await import("../src/backends/harmonyos.mjs");
  const b = mod.create({ exec });
  const r = await b.left_click({ target: { x: 123.6, y: 45.2 } });
  assert.equal(r.action_sent, true);
  const ui = calls.find((c) => c.args[0] === "uitest");
  assert.deepEqual(ui.args, ["uitest", "uiInput", "click", "124", "45"]);
});

test("harmony: clipboard and select_text fail closed with named reasons", async () => {
  const { exec } = harmonyFixtureExec();
  const mod = await import("../src/backends/harmonyos.mjs");
  const b = mod.create({ exec });
  await assert.rejects(() => b.read_clipboard(), /not exposed by hdc/);
  await assert.rejects(() => b.select_text({}), /not exposed by uitest/);
  assert.throws(() => b.key({ text: "cmd+c" }), /unsupported key/);
});

test("linux: probe reports the session and names missing tools (fail-closed)", async () => {
  const mod = await import("../src/backends/linux.mjs");
  const b = mod.create({ exec: (await import("../src/remote-runtime.mjs")).exec });
  const p = await b.probe();
  assert.equal(p.platform, "linux");
  assert.ok(["x11", "wayland", null].includes(p.session));
  assert.equal(typeof p.missing.length, "number");
  // On a host with no display session, screenshot must fail closed.
  if (p.session === null) {
    await assert.rejects(() => b.screenshot({}), /X11|Wayland/);
  }
});

test("win32: module loads with the full backend surface", async () => {
  const mod = await import("../src/backends/win32.mjs");
  assert.equal(typeof mod.create, "function");
  if (process.platform !== "win32") {
    const b = mod.create();
    for (const m of ["probe", "screenshot", "left_click", "type", "key", "recordingStart", "get_app_state", "set_value"]) {
      assert.equal(typeof b[m], "function", `win32 backend missing ${m}`);
    }
  }
});

test("remote agent refuses tools outside the allow-list", async () => {
  const { run } = await import("../src/exec.mjs");
  const sentinel = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "cu-agent-deny-")), "x");
  const payload = Buffer.from(JSON.stringify({ tool: "write_file", args: { path: sentinel } })).toString("base64");
  const r = await run("node", [new URL("../agent.mjs", import.meta.url).pathname, payload]);
  const reply = JSON.parse(r.stdout.trim());
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, "tool_not_allowed");
  assert.ok(!fs.existsSync(sentinel));
});

test("remote agent answers the platform probe", async () => {
  const { run } = await import("../src/exec.mjs");
  const payload = Buffer.from(JSON.stringify({ tool: "platform" })).toString("base64");
  const r = await run("node", [new URL("../agent.mjs", import.meta.url).pathname, payload]);
  const reply = JSON.parse(r.stdout.trim());
  assert.equal(reply.ok, true);
  assert.equal(reply.platform, process.platform);
});
