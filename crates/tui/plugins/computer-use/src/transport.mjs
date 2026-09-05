// Transport: turn a registered computer into an executor.
//  - local: spawn directly
//  - ssh:   run the codewhale-cu remote agent over ssh (args travel as base64 JSON,
//           so no tool argument can ever become remote shell syntax)
//  - hdc:   HarmonyOS device over `hdc` shell / file push-pull
import { run, runOk, ExecError } from "./exec.mjs";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import url from "node:url";

const __dirname = path.dirname(url.fileURLToPath(import.meta.url));
export const PLUGIN_ROOT = path.resolve(__dirname, "..");

export function b64(obj) {
  return Buffer.from(JSON.stringify(obj), "utf8").toString("base64");
}

/**
 * Validate a remote-side filesystem path we construct ourselves.
 * Blocks shell metacharacters and traversal outside the agent dir.
 */
export function safeRemotePath(p) {
  if (typeof p !== "string" || !/^[A-Za-z0-9.][A-Za-z0-9/._-]{0,511}$/.test(p) || p.includes("..")) {
    throw new ExecError(`refusing unsafe remote path: ${JSON.stringify(p)}`);
  }
  return p;
}

/**
 * Local executor bound to a platform backend name.
 * All backends receive this shape.
 */
export function localExec() {
  return {
    kind: "local",
    run,
    runOk,
    async readFile(p) { return fs.promises.readFile(p); },
    async writeFile(p, data) { return fs.promises.writeFile(p, data); },
    tmpFile(prefix) {
      return path.join(fs.mkdtempSync(path.join(os.tmpdir(), prefix)), "out");
    },
  };
}

/** ssh executor: speaks to the remote agent installed by installRemoteAgent(). */
export function sshExec(computer) {
  const userHost = computer.user ? `${computer.user}@${computer.host}` : computer.host;
  const portArgs = computer.port ? ["-p", String(computer.port)] : [];
  const remoteAgent = safeRemotePath(computer.agentPath ?? ".codewhale-cu/agent/agent.mjs");
  const base = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "StrictHostKeyChecking=accept-new", ...portArgs, userHost];
  return {
    kind: "ssh",
    base,
    userHost,
    remoteAgent,
    run(cmd, args = [], opts = {}) {
      // Local side commands (e.g. ssh itself) run directly.
      return run(cmd, args, opts);
    },
    async remote(request, opts = {}) {
      const r = await run("ssh", [...base, "node", remoteAgent, b64({ args: request.args ?? {}, tool: request.tool, nonce: crypto.randomBytes(6).toString("hex") })], {
        timeoutMs: opts.timeoutMs ?? 25_000,
      });
      if (r.timedOut) throw new ExecError(`ssh ${userHost}: timed out`, r);
      if (r.code !== 0) throw new ExecError(`ssh ${userHost} exited ${r.code}: ${r.stderr.trim().slice(0, 400)}`, r);
      // The agent prints exactly one JSON line; anything before it is MOTD noise.
      const line = r.stdout.trim().split("\n").filter((l) => l.startsWith("{")).pop();
      const reply = line ? JSON.parse(line) : null;
      if (!reply) throw new ExecError(`ssh ${userHost}: agent returned no JSON receipt`, r);
      return reply;
    },
  };
}

/** Push the self-contained remote agent + src tree to an ssh computer. */
export async function installRemoteAgent(computer) {
  const ex = sshExec(computer);
  const srcDir = path.join(PLUGIN_ROOT, "src");
  const rels = ["agent.mjs"];
  for (const dir of ["", "backends"]) {
    const full = path.join(srcDir, dir);
    for (const f of fs.readdirSync(full)) {
      if (f.endsWith(".mjs")) rels.push(`src/${dir ? dir + "/" : ""}${f}`);
    }
  }
  const marker = ".codewhale-cu/agent";
  let r = await run("ssh", [...ex.base, "mkdir", "-p", `${marker}/src/backends`], { timeoutMs: 15_000 });
  if (r.code !== 0) throw new ExecError(`ssh ${ex.userHost}: mkdir failed: ${r.stderr.trim().slice(0, 300)}`, r);
  for (const rel of rels) {
    const localPath = rel === "agent.mjs" ? path.join(PLUGIN_ROOT, "agent.mjs") : path.join(srcDir, rel.slice(4));
    const dest = safeRemotePath(`${marker}/${rel}`);
    r = await run("scp", [...(computer.port ? ["-P", String(computer.port)] : []), localPath, `${ex.userHost}:${dest}`], { timeoutMs: 30_000 });
    if (r.code !== 0) throw new ExecError(`scp ${rel} failed: ${r.stderr.trim().slice(0, 300)}`, r);
  }
  // Probe remote platform via the agent itself.
  const reply = await ex.remote({ tool: "platform" });
  return { installed: rels.length, remotePlatform: reply.platform, agentPath: `${marker}/agent.mjs` };
}

/** hdc (HarmonyOS) executor. Commands run on-device; files pull to local tmp. */
export function hdcExec(computer) {
  const targetArgs = computer.target ? ["-t", computer.target] : [];
  const shell = (args, opts = {}) => run("hdc", [...targetArgs, "shell", ...args], opts);
  return {
    kind: "hdc",
    targetArgs,
    run,
    runOk,
    shell,
    async pullFile(remotePath, localPath, opts = {}) {
      safeRemotePath(remotePath);
      const r = await run("hdc", [...targetArgs, "file", "recv", remotePath, localPath], opts);
      if (r.code !== 0) throw new ExecError(`hdc file recv failed: ${r.stderr.trim().slice(0, 300)}`, r);
      return localPath;
    },
    async readFile(remotePath, opts = {}) {
      const tmp = path.join(os.tmpdir(), `cu-hdc-${crypto.randomBytes(4).toString("hex")}`);
      await this.pullFile(remotePath, tmp, opts);
      const data = await fs.promises.readFile(tmp);
      await fs.promises.rm(path.dirname(tmp), { recursive: true, force: true }).catch(() => {});
      return data;
    },
  };
}

export async function executorFor(computer) {
  if (computer.transport === "local") return localExec();
  if (computer.transport === "ssh") return sshExec(computer);
  if (computer.transport === "hdc") return hdcExec(computer);
  throw new ExecError(`unknown transport ${computer.transport}`);
}

/**
 * Map a computer to its backend module. Local platform is fixed; ssh
 * computers may carry platformHint (probed at registration).
 */
export async function backendFor(computer) {
  let platform = computer.platform ?? computer.platformHint;
  if (!platform) {
    if (computer.transport === "local") platform = process.platform;
    else if (computer.transport === "hdc") platform = "harmonyos";
    else platform = "linux"; // conservative default for ssh; registration probes it
  }
  const mod = await import(`./backends/${platform}.mjs`);
  const exec = await executorFor(computer);
  return { backend: mod.create({ exec, computer, platform }), platform };
}
