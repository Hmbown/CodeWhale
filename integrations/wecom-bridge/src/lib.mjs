import { readFile, writeFile, mkdir, rename } from "node:fs/promises";
import path from "node:path";

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

export function requiredEnv(name) {
  const value = process.env[name];
  if (!value || !value.trim()) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

export function parseTextContent(content) {
  if (typeof content !== "string") return "";
  try {
    const parsed = JSON.parse(content);
    if (typeof parsed.text === "string") return parsed.text;
  } catch {
    return content;
  }
  return content;
}

export function incomingIdentity(body) {
  const from = body?.from || {};
  const chatId = body.chatid || (body.chattype === "single" && from.userid ? `single:${from.userid}` : "");
  return {
    chatId,
    messageId: body.msgid || "",
    chatType: body.chattype || "single",
    userId: from.userid || "",
    aibotId: body.aibotid || ""
  };
}

export function isAllowed(identity, allowlist, allowUnlisted = false) {
  if (allowUnlisted) return true;
  const allowed = new Set(allowlist);
  return [identity.chatId, identity.userId].filter(Boolean).some((id) => allowed.has(id));
}

export function pairingRefusalText(identity) {
  return [
    "This chat is not in WECOM_CHAT_ALLOWLIST.",
    `chat_id=${identity.chatId}`,
    identity.userId ? `user_id=${identity.userId}` : ""
  ]
    .filter(Boolean)
    .join("\n");
}

export function stripGroupPrefix(text, { chatType, requirePrefix, prefix }) {
  const trimmed = String(text || "").trim();
  if (!trimmed) return { accepted: false, text: "" };
  if (!requirePrefix || chatType === "single") {
    return { accepted: true, text: trimmed };
  }
  const marker = prefix || "/ds";
  if (trimmed === marker) return { accepted: true, text: "/help" };
  if (trimmed.startsWith(`${marker} `)) {
    return { accepted: true, text: trimmed.slice(marker.length).trim() };
  }
  return { accepted: false, text: "" };
}

export function parseCommand(text) {
  const trimmed = String(text || "").trim();
  if (!trimmed.startsWith("/")) return { name: "prompt", args: trimmed };
  const [head, ...rest] = trimmed.split(/\s+/);
  return {
    name: head.slice(1).toLowerCase(),
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

export function commandAction(command) {
  switch (command.name) {
    case "help":
      return { kind: "help" };
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
    default:
      return {
        kind: "prompt",
        prompt: `/${command.name}${command.args ? ` ${command.args}` : ""}`
      };
  }
}

export function preservedChatStateFields(state = {}) {
  const preserved = {};
  if (Object.prototype.hasOwnProperty.call(state || {}, "model")) {
    preserved.model = state.model || null;
  }
  return preserved;
}

export function splitMessage(text, maxChars = 3500) {
  const value = String(text || "");
  const chars = Array.from(value);
  if (chars.length <= maxChars) return value ? [value] : [];
  const chunks = [];
  let cursor = 0;
  while (cursor < chars.length) {
    chunks.push(chars.slice(cursor, cursor + maxChars).join(""));
    cursor += maxChars;
  }
  return chunks;
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
  return {
    turnId: runningTurn.id || state.activeTurnId || "",
    message: `Thread already has active turn ${
      runningTurn.id || state.activeTurnId || "(unknown)"
    }. Wait for it to finish or send /interrupt.`
  };
}

export function helpText() {
  return [
    "CodeWhale 企业微信桥接命令:",
    "/help - 显示帮助",
    "/status - runtime 和工作区状态",
    "/threads - 最近的 runtime 线程",
    "/new - 为此聊天创建新线程",
    "/resume <thread_id> - 绑定到此聊天的现有线程",
    "/model <name|default> - 设置或重置聊天模型",
    "/interrupt - 中断活动 turn",
    "/compact - 压缩当前线程",
    "/allow <approval_id> [remember] - 批准待处理的工具调用",
    "/deny <approval_id> - 拒绝待处理的工具调用",
    "",
    "其他所有内容均作为 CodeWhale 提示发送。"
  ].join("\n");
}

export class ThreadStore {
  static async open(filePath) {
    const store = new ThreadStore(filePath);
    await store.load();
    return store;
  }

  constructor(filePath) {
    this.filePath = filePath;
    this.data = { chats: {} };
  }

  async load() {
    try {
      const raw = await readFile(this.filePath, "utf8");
      this.data = JSON.parse(raw);
      if (!this.data.chats) this.data.chats = {};
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
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

  async save() {
    const dir = path.dirname(this.filePath);
    await mkdir(dir, { recursive: true });
    const tmp = `${this.filePath}.tmp`;
    await writeFile(tmp, `${JSON.stringify(this.data, null, 2)}\n`);
    await rename(tmp, this.filePath);
  }
}

export function validateBridgeConfig(env) {
  const errors = [];
  const warnings = [];
  const info = [];
  const add = (list, code, message) => list.push({ code, message });

  for (const key of ["WECOM_BOT_ID", "WECOM_BOT_SECRET"]) {
    const value = cleanEnvValue(env[key]);
    if (!value) {
      add(errors, "missing_required", `${key} is required`);
    } else if (isPlaceholderValue(value)) {
      add(errors, "placeholder_value", `${key} still contains a placeholder value`);
    }
  }

  const runtimeUrl = cleanEnvValue(env.CODEWHALE_RUNTIME_URL || "http://127.0.0.1:7878");
  try {
    const parsed = new URL(runtimeUrl);
    if (!["http:", "https:"].includes(parsed.protocol)) {
      add(errors, "invalid_runtime_url", "CODEWHALE_RUNTIME_URL must use http or https");
    }
  } catch {
    add(errors, "invalid_runtime_url", "CODEWHALE_RUNTIME_URL is not a valid URL");
  }

  const runtimeToken = cleanEnvValue(env.CODEWHALE_RUNTIME_TOKEN);
  if (!runtimeToken) {
    add(errors, "missing_runtime_token", "CODEWHALE_RUNTIME_TOKEN is required");
  } else if (isPlaceholderValue(runtimeToken)) {
    add(errors, "placeholder_runtime_token", "CODEWHALE_RUNTIME_TOKEN is still a placeholder");
  }

  const allowUnlisted = parseBool(env.WECOM_ALLOW_UNLISTED, false);
  const allowlist = parseList(env.WECOM_CHAT_ALLOWLIST);

  if (!allowlist.length && allowUnlisted) {
    add(warnings, "pairing_mode_open", "WECOM_ALLOW_UNLISTED=true leaves first-pairing mode open");
  } else if (!allowlist.length) {
    add(warnings, "not_paired", "WECOM_CHAT_ALLOWLIST is empty; all chats will be refused");
  }

  return { ok: errors.length === 0, errors, warnings, info };
}

export function formatValidationReport(result) {
  const lines = ["WeCom bridge config validation"];
  for (const item of result.errors) lines.push(`[fail] ${item.message}`);
  for (const item of result.warnings) lines.push(`[warn] ${item.message}`);
  for (const item of result.info) lines.push(`[info] ${item.message}`);
  if (result.ok) lines.push("[ok] No blocking config errors found");
  return lines.join("\n");
}
