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
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
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

export function telegramIdentity(update) {
  const message = update?.message || update?.edited_message || {};
  const chat = message.chat || {};
  const from = message.from || {};
  const username = from.username ? `@${from.username}` : "";
  return {
    updateId: update?.update_id ?? null,
    chatId: chat.id != null ? String(chat.id) : "",
    messageId: message.message_id != null ? String(message.message_id) : "",
    chatType: chat.type || "",
    userId: from.id != null ? String(from.id) : "",
    username,
    firstName: from.first_name || "",
    text: typeof message.text === "string" ? message.text : "",
    isBot: Boolean(from.is_bot)
  };
}

export function isGroupChat(chatType) {
  return chatType === "group" || chatType === "supergroup";
}

export function isAllowed(identity, allowlist, allowUnlisted = false) {
  if (allowUnlisted) return true;
  const allowed = new Set(allowlist);
  return [identity.chatId, identity.userId, identity.username]
    .filter(Boolean)
    .some((id) => allowed.has(id));
}

export function pairingRefusalText(identity) {
  return [
    "This Telegram chat is not in TELEGRAM_CHAT_ALLOWLIST.",
    `chat_id=${identity.chatId}`,
    identity.userId ? `user_id=${identity.userId}` : "",
    identity.username ? `username=${identity.username}` : "",
    "",
    "For first pairing, add one of those IDs to TELEGRAM_CHAT_ALLOWLIST, or temporarily set TELEGRAM_ALLOW_UNLISTED=true."
  ]
    .filter(Boolean)
    .join("\n");
}

export function stripGroupPrefix(text, { chatType, requirePrefix, prefix }) {
  const trimmed = String(text || "").trim();
  if (!trimmed) return { accepted: false, text: "" };
  if (!requirePrefix || !isGroupChat(chatType)) {
    return { accepted: true, text: trimmed };
  }
  const marker = prefix || "/cw";
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
  const name = head
    .slice(1)
    .split("@")[0]
    .toLowerCase();
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

export function commandAction(command) {
  switch (command.name) {
    case "start":
    case "help":
      return { kind: "help" };
    case "menu":
      return { kind: "menu" };
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
      return {
        kind: "prompt",
        prompt: `/${command.name}${command.args ? ` ${command.args}` : ""}`
      };
  }
}

export function controlKeyboard() {
  return {
    inline_keyboard: [
      [
        { text: "Status", callback_data: "cw:status" },
        { text: "New thread", callback_data: "cw:new" }
      ],
      [
        { text: "Threads", callback_data: "cw:threads" },
        { text: "Interrupt", callback_data: "cw:interrupt" }
      ],
      [
        { text: "Compact", callback_data: "cw:compact" },
        { text: "Reset model", callback_data: "cw:model:default" }
      ],
      [{ text: "Help", callback_data: "cw:help" }]
    ]
  };
}

export function activeTurnKeyboard() {
  return {
    inline_keyboard: [
      [
        { text: "Status", callback_data: "cw:status" },
        { text: "Interrupt", callback_data: "cw:interrupt" }
      ],
      [{ text: "Threads", callback_data: "cw:threads" }]
    ]
  };
}

export function approvalKeyboard(actionToken) {
  return {
    inline_keyboard: [
      [
        { text: "Allow once", callback_data: `cw:act:${actionToken}` },
        { text: "Allow + remember", callback_data: `cw:act:${actionToken}:remember` }
      ],
      [{ text: "Deny", callback_data: `cw:act:${actionToken}:deny` }]
    ]
  };
}

export function threadListKeyboard(threadActions) {
  const rows = [];
  for (const action of threadActions.slice(0, 8)) {
    rows.push([{ text: action.label, callback_data: `cw:act:${action.token}` }]);
  }
  rows.push([{ text: "New thread", callback_data: "cw:new" }]);
  return { inline_keyboard: rows };
}

export function callbackAction(data) {
  const value = String(data || "");
  switch (value) {
    case "cw:status":
      return { kind: "status" };
    case "cw:new":
      return { kind: "new_thread" };
    case "cw:threads":
      return { kind: "threads" };
    case "cw:interrupt":
      return { kind: "interrupt" };
    case "cw:compact":
      return { kind: "compact" };
    case "cw:help":
      return { kind: "help" };
    case "cw:model:default":
      return { kind: "set_model", modelName: "default" };
    default:
      break;
  }
  if (value.startsWith("cw:act:")) {
    const [, , token, suffix] = value.split(":", 4);
    return { kind: "stored_action", token: token || "", suffix: suffix || "" };
  }
  return null;
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

export function telegramRetryDelayMs(error, fallbackMs = 3000) {
  const retryAfter = Number(error?.parameters?.retry_after || 0);
  if (Number.isFinite(retryAfter) && retryAfter > 0) {
    return Math.min(retryAfter * 1000, 60000);
  }
  return fallbackMs;
}

export function looksLikePollingConflict(error) {
  const text = String(error?.description || error?.message || "").toLowerCase();
  return error?.errorCode === 409 || text.includes("terminated by other getupdates request");
}

export function validateBridgeConfig(env, options = {}) {
  const runtimeEnv = options.runtimeEnv || null;
  const workspaceRoot = options.workspaceRoot || "";
  const errors = [];
  const warnings = [];
  const info = [];
  const add = (list, code, message) => list.push({ code, message });

  const botToken = envFirst(env, "TELEGRAM_BOT_TOKEN");
  if (!botToken) {
    add(errors, "missing_required", "TELEGRAM_BOT_TOKEN is required");
  } else if (isPlaceholderValue(botToken)) {
    add(errors, "placeholder_value", "TELEGRAM_BOT_TOKEN still contains a placeholder value");
  }

  const runtimeUrl = envFirst(env, "CODEWHALE_RUNTIME_URL", "DEEPSEEK_RUNTIME_URL") || "http://127.0.0.1:7878";
  try {
    const parsed = new URL(runtimeUrl);
    const localHosts = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);
    if (!["http:", "https:"].includes(parsed.protocol)) {
      add(errors, "invalid_runtime_url", "CODEWHALE_RUNTIME_URL must use http or https");
    }
    if (!localHosts.has(parsed.hostname) && options.requireLocalRuntime !== false) {
      add(errors, "remote_runtime_url", "CODEWHALE_RUNTIME_URL should point at localhost on a VM deployment");
    }
  } catch {
    add(errors, "invalid_runtime_url", "CODEWHALE_RUNTIME_URL is not a valid URL");
  }

  const runtimeToken = envFirst(env, "CODEWHALE_RUNTIME_TOKEN", "DEEPSEEK_RUNTIME_TOKEN");
  if (!runtimeToken) {
    add(errors, "missing_required", "CODEWHALE_RUNTIME_TOKEN is required");
  } else if (isPlaceholderValue(runtimeToken)) {
    add(errors, "placeholder_value", "CODEWHALE_RUNTIME_TOKEN still contains a placeholder value");
  }

  const workspace = envFirst(env, "CODEWHALE_WORKSPACE", "DEEPSEEK_WORKSPACE");
  if (workspace && !workspace.startsWith("/")) {
    add(errors, "relative_workspace", "CODEWHALE_WORKSPACE must be an absolute path");
  }
  if (
    workspace &&
    workspaceRoot &&
    workspace !== workspaceRoot &&
    !workspace.startsWith(`${workspaceRoot}/`)
  ) {
    add(warnings, "workspace_root", `CODEWHALE_WORKSPACE is outside ${workspaceRoot}`);
  }

  const threadMapPath = envFirst(env, "TELEGRAM_THREAD_MAP_PATH");
  if (threadMapPath && !threadMapPath.startsWith("/")) {
    add(errors, "relative_thread_map", "TELEGRAM_THREAD_MAP_PATH must be an absolute path");
  }

  const allowGroups = parseBool(env.TELEGRAM_ALLOW_GROUPS, false);
  const requirePrefix = parseBool(env.TELEGRAM_REQUIRE_PREFIX_IN_GROUP, true);
  const allowUnlisted = parseBool(
    envFirst(env, "TELEGRAM_ALLOW_UNLISTED", "CODEWHALE_ALLOW_UNLISTED", "DEEPSEEK_ALLOW_UNLISTED"),
    false
  );
  const allowlist = parseList(
    envFirst(env, "TELEGRAM_CHAT_ALLOWLIST", "CODEWHALE_CHAT_ALLOWLIST", "DEEPSEEK_CHAT_ALLOWLIST")
  );

  if (!allowlist.length && allowUnlisted) {
    add(warnings, "pairing_mode_open", "TELEGRAM_ALLOW_UNLISTED=true leaves first-pairing mode open");
  } else if (!allowlist.length) {
    add(warnings, "not_paired", "TELEGRAM_CHAT_ALLOWLIST is empty; all chats will be refused");
  }
  if (allowGroups && allowUnlisted) {
    add(errors, "open_group_control", "Group control cannot be enabled while unlisted chats are allowed");
  }
  if (allowGroups && !requirePrefix) {
    add(warnings, "group_without_prefix", "Group control is enabled without requiring TELEGRAM_GROUP_PREFIX");
  }
  if (!allowGroups) {
    add(info, "dm_only", "Direct-message control is enabled; group chats are disabled");
  }

  const maxReplyChars = Number(env.TELEGRAM_MAX_REPLY_CHARS || 3500);
  if (!Number.isFinite(maxReplyChars) || maxReplyChars < 100 || maxReplyChars > 4096) {
    add(errors, "invalid_max_reply_chars", "TELEGRAM_MAX_REPLY_CHARS must be between 100 and 4096");
  }
  const pollTimeout = Number(env.TELEGRAM_POLL_TIMEOUT_SECONDS || 50);
  if (!Number.isFinite(pollTimeout) || pollTimeout < 1 || pollTimeout > 60) {
    add(errors, "invalid_poll_timeout", "TELEGRAM_POLL_TIMEOUT_SECONDS must be between 1 and 60");
  }
  const turnTimeoutMs = Number(envFirst(env, "CODEWHALE_TURN_TIMEOUT_MS", "DEEPSEEK_TURN_TIMEOUT_MS") || 900000);
  if (!Number.isFinite(turnTimeoutMs) || turnTimeoutMs < 1000) {
    add(errors, "invalid_turn_timeout", "CODEWHALE_TURN_TIMEOUT_MS must be at least 1000");
  }

  if (runtimeEnv) {
    const runtimeFileToken = envFirst(runtimeEnv, "CODEWHALE_RUNTIME_TOKEN", "DEEPSEEK_RUNTIME_TOKEN");
    if (!runtimeFileToken) {
      add(errors, "missing_runtime_token", "runtime.env is missing CODEWHALE_RUNTIME_TOKEN");
    } else if (isPlaceholderValue(runtimeFileToken)) {
      add(errors, "placeholder_runtime_token", "runtime.env CODEWHALE_RUNTIME_TOKEN is still a placeholder");
    } else if (runtimeToken && runtimeToken !== runtimeFileToken) {
      add(errors, "token_mismatch", "Runtime and bridge token values do not match");
    }

    const provider = envFirst(runtimeEnv, "CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER");
    if (!provider) {
      add(warnings, "missing_provider", "runtime.env does not set CODEWHALE_PROVIDER");
    }

    const runtimePort = Number(envFirst(runtimeEnv, "CODEWHALE_RUNTIME_PORT", "DEEPSEEK_RUNTIME_PORT") || 7878);
    if (!Number.isInteger(runtimePort) || runtimePort <= 0 || runtimePort > 65535) {
      add(errors, "invalid_runtime_port", "runtime port must be a valid TCP port");
    }
  }

  return {
    ok: errors.length === 0,
    errors,
    warnings,
    info
  };
}

export function formatValidationReport(result) {
  const lines = ["Telegram bridge config validation"];
  for (const item of result.errors) lines.push(`[fail] ${item.message}`);
  for (const item of result.warnings) lines.push(`[warn] ${item.message}`);
  for (const item of result.info) lines.push(`[info] ${item.message}`);
  if (result.ok) lines.push("[ok] No blocking config errors found");
  return lines.join("\n");
}

export function helpText() {
  return [
    "CodeWhale Telegram bridge commands:",
    "/menu - open tappable controls",
    "/help - show this help",
    "/status - runtime and workspace status",
    "/threads - recent runtime threads",
    "/new - create a new thread for this chat",
    "/resume <thread_id> - bind this chat to an existing thread",
    "/model <name|default> - set or reset this chat's model",
    "/interrupt - interrupt the active turn",
    "/compact - compact the current thread",
    "/allow <approval_id> [remember] - approve a pending tool call",
    "/deny <approval_id> - deny a pending tool call",
    "",
    "Anything else is sent as a CodeWhale prompt."
  ].join("\n");
}
