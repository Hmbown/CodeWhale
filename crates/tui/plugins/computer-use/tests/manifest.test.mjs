// Packaging tests: the plugin bundle must satisfy the Codewhale Engine's
// Agent Plugins v1 contract (plugin.json + sibling mcp.json), because the
// Engine — not this repo — installs it.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const ROOT = path.dirname(path.dirname(url.fileURLToPath(import.meta.url)));
const read = (name) => JSON.parse(fs.readFileSync(path.join(ROOT, name), "utf8"));

test("plugin.json is an Agent Plugins v1 manifest with the Codewhale extension", () => {
  const manifest = read("plugin.json");
  assert.equal(manifest.$schema, "https://agent-plugins.org/schemas/plugin.json");
  assert.equal(manifest.name, "computer-use");
  assert.match(manifest.version, /^\d+\.\d+\.\d+$/);

  const ext = manifest.extensions["net.codewhale"];
  assert.deepEqual(ext.commands, { path: "commands" });
  assert.deepEqual(ext.skills, { path: "skills" });
  assert.deepEqual(ext.when.binaries, ["node"]);
  // HarmonyOS is a target device, never a host that runs the server.
  assert.deepEqual([...ext.when.os].sort(), ["linux", "macos", "windows"]);

  const rootKeys = ["$schema", "name", "version", "description", "author", "homepage", "repository", "license", "keywords", "extensions"];
  for (const key of Object.keys(manifest)) assert.ok(rootKeys.includes(key), `unknown root key ${key}`);
});

test("mcp.json declares one stdio server on a contained relative path", () => {
  const servers = read("mcp.json").mcpServers;
  assert.deepEqual(Object.keys(servers), ["computer"]);
  const server = servers.computer;
  assert.equal(server.type, "stdio");
  assert.equal(server.command, "node");
  assert.equal(server.cwd, ".");
  for (const arg of server.args) {
    assert.ok(!path.isAbsolute(arg), `${arg} must be relative`);
    assert.ok(!arg.split("/").includes(".."), `${arg} must not escape the plugin root`);
  }
  assert.ok(fs.existsSync(path.join(ROOT, server.args[0])), "server entrypoint exists");
});

test("declared components exist and every skill is named for its directory", () => {
  assert.ok(fs.existsSync(path.join(ROOT, "commands/computer.md")));
  const skills = fs.readdirSync(path.join(ROOT, "skills"));
  assert.deepEqual(skills.sort(), ["computer-use", "recording"]);
  for (const skill of skills) {
    const text = fs.readFileSync(path.join(ROOT, "skills", skill, "SKILL.md"), "utf8");
    assert.match(text, new RegExp(`^---\\nname: ${skill}\\ndescription: .+`), `${skill} frontmatter`);
  }
});
