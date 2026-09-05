// Computer registry: named computers (local, ssh, hdc) with a sticky active
// computer. Every codewhale-cu tool defaults to the active computer; passing
// `computer` on any tool switches to it first (switch-by-use).
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

export class RegistryError extends Error {
  constructor(code, message) { super(message); this.name = "RegistryError"; this.code = code; }
}

const ID_RE = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$/;

export function stateDir() {
  return process.env.CODEWHALE_CU_STATE_DIR || path.join(os.homedir(), ".codewhale-cu");
}

function registryPath() { return path.join(stateDir(), "computers.json"); }

const PLATFORM = process.platform; // darwin | linux | win32

function localEntry() {
  return {
    id: "local",
    transport: "local",
    platform: PLATFORM,
    label: `This ${PLATFORM === "darwin" ? "Mac" : PLATFORM === "win32" ? "Windows PC" : "Linux machine"}`,
    registeredAt: new Date().toISOString(),
  };
}

export function load() {
  try {
    const raw = JSON.parse(fs.readFileSync(registryPath(), "utf8"));
    if (!raw || typeof raw !== "object" || !raw.computers) throw new Error("bad shape");
    return raw;
  } catch {
    const fresh = { version: 1, active: "local", computers: { local: localEntry() } };
    return fresh;
  }
}

export function save(reg) {
  fs.mkdirSync(stateDir(), { recursive: true });
  const tmp = registryPath() + ".tmp";
  fs.writeFileSync(tmp, JSON.stringify(reg, null, 2) + "\n");
  fs.renameSync(tmp, registryPath());
  return reg;
}

export function list() {
  const reg = load();
  // Keep the local entry truthful even if the state file is stale.
  reg.computers.local = { ...(reg.computers.local ?? {}), ...localEntry() };
  if (!reg.computers[reg.active]) reg.active = "local";
  return reg;
}

export function get(id) {
  const reg = list();
  const c = reg.computers[id];
  if (!c) throw new RegistryError("unknown_computer", `no computer registered with id "${id}". Use computer_list.`);
  return c;
}

export function active() { return get(list().active); }

/** Switch the active computer. Returns the computer entry. */
export function switchTo(id) {
  const c = get(id); // throws unknown_computer
  const reg = load();
  reg.active = c.id;
  save(reg);
  return c;
}

/** Register or update a computer. Returns the entry. */
export function register({ id, transport, label, ...rest }) {
  if (!id || !ID_RE.test(id)) throw new RegistryError("invalid_id", "computer id must match " + ID_RE);
  if (!["local", "ssh", "hdc"].includes(transport)) {
    throw new RegistryError("invalid_transport", "transport must be one of: local, ssh, hdc");
  }
  if (id === "local" && transport !== "local") {
    throw new RegistryError("reserved_id", '"local" is reserved for this machine');
  }
  if (transport === "ssh") {
    if (!rest.host || !/^[A-Za-z0-9._-]+$/.test(rest.host)) {
      throw new RegistryError("invalid_host", "ssh computers need a valid host (letters, digits, dot, dash, underscore)");
    }
    if (rest.port != null && (!Number.isInteger(rest.port) || rest.port < 1 || rest.port > 65535)) {
      throw new RegistryError("invalid_port", "port must be an integer in 1..65535");
    }
    if (rest.user != null && !/^[a-zA-Z0-9._-]+$/.test(rest.user)) {
      throw new RegistryError("invalid_user", "user must be a plain name");
    }
  }
  if (transport === "hdc") {
    if (rest.target != null && !/^[A-Za-z0-9._-]*$/.test(rest.target)) {
      throw new RegistryError("invalid_target", "hdc target key contains invalid characters");
    }
    rest.platform = "harmonyos";
  }
  const reg = load();
  const prev = reg.computers[id];
  reg.computers[id] = {
    ...prev,
    id,
    transport,
    label: label ?? prev?.label ?? id,
    registeredAt: prev?.registeredAt ?? new Date().toISOString(),
    ...rest,
  };
  save(reg);
  return reg.computers[id];
}

export function remove(id) {
  if (id === "local") throw new RegistryError("reserved_id", '"local" cannot be removed');
  const reg = load();
  if (!reg.computers[id]) throw new RegistryError("unknown_computer", `no computer registered with id "${id}"`);
  delete reg.computers[id];
  if (reg.active === id) reg.active = "local";
  save(reg);
  return { removed: id, active: reg.active };
}
