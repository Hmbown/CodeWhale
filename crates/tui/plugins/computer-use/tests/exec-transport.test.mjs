// exec + transport safety tests.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { run, runOk, ExecError, have, trim } from "../src/exec.mjs";
import { safeRemotePath, b64, localExec, hdcExec } from "../src/transport.mjs";

test("run captures stdout/stderr and exit codes without a shell", async () => {
  const r = await run("node", ["-e", "console.log('hello'); console.error('boo')"]);
  assert.equal(r.code, 0);
  assert.equal(r.stdout.trim(), "hello");
  assert.match(r.stderr, /boo/);
});

test("run reports missing executables as code -1 with ENOENT, never throws", async () => {
  const r = await run("definitely-not-a-real-tool-xyz", ["--version"]);
  assert.equal(r.code, -1);
  assert.match(r.stderr, /ENOENT/);
});

test("runOk throws ExecError on non-zero exit and includes stderr", async () => {
  await assert.rejects(() => runOk("node", ["-e", "console.error('reason-here'); process.exit(3)"]), (e) => {
    assert.ok(e instanceof ExecError);
    assert.match(e.message, /exited 3/);
    assert.match(e.message, /reason-here/);
    return true;
  });
});

test("run enforces timeouts", async () => {
  const r = await run("node", ["-e", "setInterval(()=>{},1000)"], { timeoutMs: 300 });
  assert.equal(r.timedOut, true);
});

test("have() detects real and missing tools", async () => {
  assert.equal(await have("node"), true);
  assert.equal(await have("definitely-not-a-real-tool-xyz"), false);
});

test("safeRemotePath blocks traversal, metacharacters, and absolute escapes", () => {
  assert.equal(safeRemotePath(".codewhale-cu/agent/agent.mjs"), ".codewhale-cu/agent/agent.mjs");
  for (const bad of ["../../etc/passwd", "/etc/passwd", "a;rm -rf /", "a b", "$(id)", "a\nb", "a'b", ".codewhale-cu/../escape"]) {
    assert.throws(() => safeRemotePath(bad), ExecError, `should reject: ${bad}`);
  }
});

test("b64 round-trips JSON payloads", () => {
  const obj = { tool: "screenshot", args: { region: [0, 0, 10, 10] } };
  assert.deepEqual(JSON.parse(Buffer.from(b64(obj), "base64").toString("utf8")), obj);
});

test("localExec provides run/runOk/tmpFile", async () => {
  const ex = localExec();
  const r = await ex.run("echo", ["hi"]);
  assert.equal(r.code, 0);
  const f = ex.tmpFile("cu-test-");
  assert.ok(typeof f === "string");
});

for (const cleanupFails of [false, true]) {
  for (const outcome of ["success", "transfer failure", "read failure"]) {
    test(`hdc readFile preserves ${outcome} when cleanup ${cleanupFails ? "fails" : "succeeds"}`, async (t) => {
      // Contain even the old recursive-deletion bug inside this fixture.
      const root = await fs.mkdtemp(path.join(os.tmpdir(), "cu-hdc-test-"));
      const realRm = fs.rm.bind(fs);
      t.after(() => realRm(root, { recursive: true, force: true }));
      if (cleanupFails) {
        t.mock.method(fs, "rm", async (dir) => {
          assert.equal(path.dirname(dir), root, "cleanup stays inside its fixture");
          assert.ok(path.basename(dir).startsWith("cu-hdc-"));
          throw Object.assign(new Error("cleanup denied"), { code: "EACCES" });
        });
      }
      t.mock.method(os, "tmpdir", () => root);
      await fs.writeFile(path.join(root, "unrelated"), "keep me");
      const ex = hdcExec({});
      const transferError = new Error("transfer interrupted");
      ex.pullFile = async (remote, local) => {
        assert.equal(remote, "fixture.txt");
        if (outcome === "read failure") return;
        await fs.writeFile(local, "downloaded bytes");
        if (outcome === "transfer failure") throw transferError;
      };
      if (outcome === "success") {
        assert.equal((await ex.readFile("fixture.txt")).toString(), "downloaded bytes");
      } else {
        await assert.rejects(ex.readFile("fixture.txt"), (error) =>
          outcome === "transfer failure" ? error === transferError : error.code === "ENOENT");
      }
      assert.equal(await fs.readFile(path.join(root, "unrelated"), "utf8"), "keep me");
      if (!cleanupFails) assert.deepEqual(await fs.readdir(root), ["unrelated"]);
    });
  }
}
