import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

const DEFAULT_ACTION_TTL_MS = 24 * 60 * 60 * 1000;

function normalizeCursorValue(value, fallback = 0) {
  const number = Number(value);
  if (Number.isFinite(number) && number >= 0) return Math.floor(number);
  const fallbackNumber = Number(fallback);
  if (Number.isFinite(fallbackNumber) && fallbackNumber >= 0) return Math.floor(fallbackNumber);
  return 0;
}

async function chmodBestEffort(filePath, mode) {
  try {
    await chmod(filePath, mode);
  } catch (error) {
    if (process.platform !== "win32") throw error;
  }
}

export class ThreadStore {
  static async open(filePath, options = {}) {
    const store = new ThreadStore(filePath, options);
    await store.load();
    return store;
  }

  constructor(filePath, options = {}) {
    this.filePath = filePath;
    this.options = {
      messageLimit: options.messageLimit || 0,
      actions: options.actions === true,
      actionLimit: options.actionLimit || 200,
      actionTtlMs: options.actionTtlMs || DEFAULT_ACTION_TTL_MS,
      privateMode: options.privateMode === true
    };
    this.data = { chats: {} };
    this.saveDirty = false;
    this.savePending = null;
    this.ensureShape();
  }

  ensureShape() {
    if (!this.data || typeof this.data !== "object") this.data = {};
    if (!this.data.chats || typeof this.data.chats !== "object") this.data.chats = {};
    if (this.options.messageLimit > 0 && !Array.isArray(this.data.messages)) {
      this.data.messages = [];
    }
    if (this.options.actions && (!this.data.actions || typeof this.data.actions !== "object")) {
      this.data.actions = {};
    }
    if (this.data.cursors && typeof this.data.cursors !== "object") {
      this.data.cursors = {};
    }
  }

  async load() {
    try {
      const raw = await readFile(this.filePath, "utf8");
      this.data = JSON.parse(raw);
      this.ensureShape();
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }

  async recordMessage(messageKey) {
    if (!messageKey || this.options.messageLimit <= 0) return false;
    this.ensureShape();
    if (this.data.messages.includes(messageKey)) return true;
    this.data.messages.push(messageKey);
    this.data.messages = this.data.messages.slice(-this.options.messageLimit);
    await this.save();
    return false;
  }

  getCursor(name, fallback = 0) {
    if (!name) return normalizeCursorValue(fallback);
    this.ensureShape();
    return normalizeCursorValue(this.data.cursors?.[name], fallback);
  }

  async setCursor(name, value) {
    if (!name) return normalizeCursorValue(value);
    this.ensureShape();
    if (!this.data.cursors || typeof this.data.cursors !== "object") {
      this.data.cursors = {};
    }
    const cursor = normalizeCursorValue(value);
    if (this.data.cursors[name] === cursor) return cursor;
    this.data.cursors[name] = cursor;
    await this.save();
    return cursor;
  }

  async getChat(chatId) {
    return this.data.chats[chatId] || null;
  }

  listChats() {
    return Object.entries(this.data.chats || {});
  }

  async setChat(chatId, state) {
    this.data.chats[chatId] = state;
    await this.save();
    return state;
  }

  async patchChat(chatId, patch) {
    const current = this.data.chats[chatId] || {};
    this.data.chats[chatId] = { ...current, ...patch };
    await this.save();
    return this.data.chats[chatId];
  }

  async putAction(action) {
    if (!this.options.actions) return "";
    this.ensureShape();
    const token = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    this.data.actions[token] = {
      ...action,
      createdAt: new Date().toISOString()
    };
    this.pruneActions();
    await this.save();
    return token;
  }

  async getAction(token) {
    if (!token || !this.options.actions) return null;
    this.ensureShape();
    return this.data.actions[token] || null;
  }

  async takeAction(token) {
    const action = await this.getAction(token);
    if (action) {
      delete this.data.actions[token];
      await this.save();
    }
    return action;
  }

  pruneActions() {
    if (!this.options.actions) return;
    const cutoff = Date.now() - this.options.actionTtlMs;
    const fresh = Object.entries(this.data.actions || {}).filter(([, action]) => {
      const time = Date.parse(action.createdAt || "");
      return Number.isFinite(time) && time >= cutoff;
    });
    this.data.actions = Object.fromEntries(fresh.slice(-this.options.actionLimit));
  }

  async save() {
    // Batch bursts of small updates: saves issued while a write is in flight
    // coalesce into a single follow-up write. The returned promise resolves
    // only after this mutation is durable on disk (temp file + rename).
    this.saveDirty = true;
    if (!this.savePending) {
      this.savePending = this.flushSaves();
    }
    return this.savePending;
  }

  async flushSaves() {
    try {
      while (this.saveDirty) {
        this.saveDirty = false;
        await this.writeSnapshot();
      }
    } finally {
      this.savePending = null;
    }
  }

  async writeSnapshot() {
    const dir = path.dirname(this.filePath);
    await mkdir(dir, { recursive: true, mode: 0o700 });
    if (this.options.privateMode) await chmodBestEffort(dir, 0o700);
    const tmp = `${this.filePath}.tmp`;
    await writeFile(tmp, `${JSON.stringify(this.data, null, 2)}\n`, { mode: 0o600 });
    if (this.options.privateMode) await chmodBestEffort(tmp, 0o600);
    await rename(tmp, this.filePath);
    if (this.options.privateMode) await chmodBestEffort(this.filePath, 0o600);
  }
}

export function envFirst(env, ...names) {
  for (const name of names) {
    const value = env?.[name];
    if (value != null && String(value).trim()) return String(value).trim();
  }
  return "";
}

export function parseList(raw) {
  return String(raw || "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

export function parseBool(raw, fallback = false) {
  if (raw == null || raw === "") return fallback;
  return ["1", "true", "yes", "on"].includes(String(raw).trim().toLowerCase());
}

export function parseEnvText(raw) {
  const env = {};
  for (const line of String(raw || "").split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const normalized = trimmed.startsWith("export ") ? trimmed.slice(7).trim() : trimmed;
    const index = normalized.indexOf("=");
    if (index <= 0) continue;
    const key = normalized.slice(0, index).trim();
    let value = normalized.slice(index + 1).trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    env[key] = value;
  }
  return env;
}

export function cleanEnvValue(value) {
  return String(value ?? "").trim();
}

export function isPlaceholderValue(value) {
  const normalized = cleanEnvValue(value).toLowerCase();
  return (
    !normalized ||
    normalized.includes("replace-with") ||
    normalized.includes("xxxxxxxx") ||
    normalized === "changeme"
  );
}

export function parseTextContent(content, keys = ["text", "content"]) {
  if (typeof content !== "string") return "";
  try {
    const parsed = JSON.parse(content);
    for (const key of keys) {
      if (typeof parsed?.[key] === "string") return parsed[key];
    }
  } catch {
    return content;
  }
  return content;
}

export function stripGroupPrefix(text, { chatType, requirePrefix, prefix, directChatTypes = [] }) {
  const trimmed = String(text || "").trim();
  if (!trimmed) return { accepted: false, text: "" };
  if (!requirePrefix || directChatTypes.includes(chatType)) {
    return { accepted: true, text: trimmed };
  }
  const marker = prefix || "/ds";
  if (trimmed === marker) return { accepted: true, text: "/help" };
  if (trimmed.startsWith(`${marker} `)) {
    return { accepted: true, text: trimmed.slice(marker.length).trim() };
  }
  return { accepted: false, text: "" };
}

export function parseCommand(text, options = {}) {
  const trimmed = String(text || "").trim();
  if (!trimmed.startsWith("/")) return { name: "prompt", args: trimmed };
  const [head, ...rest] = trimmed.split(/\s+/);
  const rawName = head.slice(1);
  const name = (options.stripBotMention ? rawName.split("@")[0] : rawName).toLowerCase();
  return {
    name,
    args: rest.join(" ").trim()
  };
}

export function parseApprovalDecisionArgs(args) {
  const parts = String(args || "")
    .split(/\s+/)
    .filter(Boolean);
  return {
    approvalId: parts[0] || "",
    remember: parts.slice(1).includes("remember")
  };
}

export function commandAction(command, options = {}) {
  const allowMenu = options.allowMenu === true;
  const allowStart = options.allowStart === true;
  switch (command.name) {
    case "start":
      if (allowStart) return { kind: "help" };
      break;
    case "help":
      return { kind: "help" };
    case "menu":
      if (allowMenu) return { kind: "menu" };
      break;
    case "status":
      return { kind: "status" };
    case "threads":
      return { kind: "threads" };
    case "new":
      return { kind: "new_thread" };
    case "resume":
      return { kind: "resume", threadId: command.args };
    case "interrupt":
      return { kind: "interrupt" };
    case "compact":
      return { kind: "compact" };
    case "model":
      return { kind: "set_model", modelName: command.args };
    case "allow":
      return { kind: "approval", decision: "allow", ...parseApprovalDecisionArgs(command.args) };
    case "deny":
      return { kind: "approval", decision: "deny", ...parseApprovalDecisionArgs(command.args) };
    case "prompt":
      return { kind: "prompt", prompt: command.args };
    default:
      break;
  }
  return {
    kind: "prompt",
    prompt: `/${command.name}${command.args ? ` ${command.args}` : ""}`
  };
}

export function preservedChatStateFields(state = {}, fields = ["model"]) {
  const preserved = {};
  for (const field of fields) {
    if (Object.prototype.hasOwnProperty.call(state || {}, field)) {
      preserved[field] = state[field] || null;
    }
  }
  return preserved;
}

export function splitMessage(text, maxChars = 3500) {
  const value = String(text || "");
  const limit = Math.max(1, Math.floor(Number(maxChars) || 3500));
  // Materialize code points once; all chunking below works on index ranges
  // instead of re-running Array.from over the shrinking remainder.
  const chars = Array.from(value);
  if (chars.length <= limit) return value ? [value] : [];
  const chunks = [];
  let offset = 0;
  let openFence = null;
  while (offset < chars.length) {
    const next = takeRenderedSplitMessageChunk(chars, offset, limit, openFence);
    chunks.push(next.chunk);
    offset = next.offset;
    openFence = next.openFence;
  }
  return chunks;
}

function takeRenderedSplitMessageChunk(chars, offset, maxChars, openFence) {
  const prefix = openFence !== null ? `\`\`\`${openFence}\n` : "";
  const prefixLength = charLength(prefix);
  let payloadLimit = Math.max(1, maxChars - prefixLength);

  while (true) {
    const splitAt = splitMessageChunkEnd(chars, offset, payloadLimit);
    const payload = chars.slice(offset, splitAt).join("");
    const body = `${prefix}${payload}`;
    const nextOpenFence = updateCodeFenceState(openFence, payload);
    const suffix =
      nextOpenFence !== null && splitAt < chars.length ? (body.endsWith("\n") ? "```" : "\n```") : "";
    // prefix/suffix are ASCII fence markup, so string length == code points.
    const overflow = prefixLength + (splitAt - offset) + suffix.length - maxChars;
    if (overflow <= 0 || payloadLimit === 1) {
      return { chunk: `${body}${suffix}`, offset: splitAt, openFence: nextOpenFence };
    }
    payloadLimit = Math.max(1, payloadLimit - overflow);
  }
}

function splitMessageChunkEnd(chars, offset, maxChars) {
  if (chars.length - offset <= maxChars) return chars.length;
  return offset + preferredSplitIndex(chars, offset, maxChars);
}

function preferredSplitIndex(chars, offset, maxChars) {
  const limit = Math.min(chars.length - offset, maxChars);
  for (let i = limit - 1; i > 0; i -= 1) {
    if (chars[offset + i] === "\n") return i + 1;
  }
  for (let i = limit - 1; i > 0; i -= 1) {
    if (/\s/u.test(chars[offset + i])) return i + 1;
  }
  return limit;
}

function charLength(text) {
  let length = 0;
  for (const _ of text) length += 1;
  return length;
}

function updateCodeFenceState(openFence, text) {
  let current = openFence;
  for (const match of text.matchAll(/^```([^\n`]*)\s*$/gm)) {
    if (current === null) {
      current = match[1]?.trim() || "";
    } else {
      current = null;
    }
  }
  return current;
}

export async function readJsonSafe(response) {
  const text = await response.text();
  if (!text) return {};
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

export async function* readSse(response) {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of response.body) {
    buffer += decoder.decode(chunk, { stream: true });
    let boundary;
    while ((boundary = buffer.indexOf("\n\n")) >= 0) {
      const raw = buffer.slice(0, boundary).replace(/\r/g, "");
      buffer = buffer.slice(boundary + 2);
      const event = { event: "", data: "" };
      for (const line of raw.split("\n")) {
        if (line.startsWith("event:")) event.event = line.slice(6).trim();
        if (line.startsWith("data:")) event.data += line.slice(5).trim();
      }
      yield event;
    }
  }
}

export function createRuntimeClient({ runtimeUrl, runtimeToken }) {
  function authHeaders() {
    return { authorization: `Bearer ${runtimeToken}` };
  }

  async function runtimeJson(route, options = {}) {
    const response = await fetch(`${runtimeUrl}${route}`, {
      method: options.method || "GET",
      headers: {
        ...(options.auth === false ? {} : authHeaders()),
        ...(options.body ? { "content-type": "application/json" } : {})
      },
      body: options.body ? JSON.stringify(options.body) : undefined
    });
    const body = await readJsonSafe(response);
    if (!response.ok) {
      throw new Error(compactRuntimeError(response.status, body));
    }
    return body;
  }

  return { runtimeJson, authHeaders };
}

export function compactRuntimeError(status, body) {
  const message =
    body?.error?.message ||
    body?.message ||
    (typeof body === "string" ? body : JSON.stringify(body));
  return `Runtime API request failed (${status}): ${message}`;
}

export function latestRunningTurn(detail) {
  const turns = Array.isArray(detail?.turns) ? detail.turns : [];
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (["queued", "in_progress"].includes(turn?.status)) return turn;
  }
  return null;
}

export function activeTurnBlock(detail, state = {}) {
  const runningTurn = latestRunningTurn(detail);
  if (!runningTurn) return null;
  const activeTurnId = state?.activeTurnId || "";
  return {
    turnId: runningTurn.id || activeTurnId,
    message: `Thread already has active turn ${
      runningTurn.id || activeTurnId || "(unknown)"
    }. Wait for it to finish or send /interrupt.`
  };
}
