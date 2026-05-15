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

export function parseTextContent(content) {
  if (typeof content !== "string") return "";
  try {
    const parsed = JSON.parse(content);
    if (typeof parsed.text === "string") return parsed.text;
    if (typeof parsed.content === "string") return parsed.content;
  } catch {
    return content;
  }
  return content;
}

export function incomingIdentity(event) {
  const sender = event?.sender?.sender_id || {};
  const message = event?.message || {};
  return {
    chatId: message.chat_id || "",
    messageId: message.message_id || "",
    chatType: message.chat_type || "",
    messageType: message.message_type || "",
    openId: sender.open_id || "",
    unionId: sender.union_id || "",
    userId: sender.user_id || ""
  };
}

export function isAllowed(identity, allowlist, allowUnlisted = false) {
  if (allowUnlisted) return true;
  const allowed = new Set(allowlist);
  return [identity.chatId, identity.openId, identity.unionId, identity.userId]
    .filter(Boolean)
    .some((id) => allowed.has(id));
}

export function pairingRefusalText(identity) {
  return [
    "This chat is not in DEEPSEEK_CHAT_ALLOWLIST.",
    `chat_id=${identity.chatId}`,
    identity.openId ? `open_id=${identity.openId}` : "",
    identity.unionId ? `union_id=${identity.unionId}` : "",
    identity.userId ? `user_id=${identity.userId}` : ""
  ]
    .filter(Boolean)
    .join("\n");
}

export function stripGroupPrefix(text, { chatType, requirePrefix, prefix }) {
  const trimmed = String(text || "").trim();
  if (!trimmed) return { accepted: false, text: "" };
  if (!requirePrefix || chatType === "p2p") {
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

export function splitMessage(text, maxChars = 3500) {
  const value = String(text || "");
  if (value.length <= maxChars) return value ? [value] : [];
  const chunks = [];
  let cursor = 0;
  while (cursor < value.length) {
    chunks.push(value.slice(cursor, cursor + maxChars));
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

export function validateBridgeConfig(env, options = {}) {
  const runtimeEnv = options.runtimeEnv || null;
  const workspaceRoot = options.workspaceRoot || "";
  const errors = [];
  const warnings = [];
  const info = [];
  const add = (list, code, message) => list.push({ code, message });

  for (const key of [
    "FEISHU_APP_ID",
    "FEISHU_APP_SECRET",
    "DEEPSEEK_RUNTIME_URL",
    "DEEPSEEK_RUNTIME_TOKEN",
    "DEEPSEEK_WORKSPACE",
    "FEISHU_THREAD_MAP_PATH"
  ]) {
    const value = cleanEnvValue(env[key]);
    if (!value) {
      add(errors, "missing_required", `${key} is required`);
    } else if (isPlaceholderValue(value)) {
      add(errors, "placeholder_value", `${key} still contains a placeholder value`);
    }
  }

  const domain = cleanEnvValue(env.FEISHU_DOMAIN || "feishu").toLowerCase();
  if (!["feishu", "lark"].includes(domain) && !/^https:\/\/open\./.test(domain)) {
    add(errors, "invalid_domain", "FEISHU_DOMAIN must be feishu, lark, or an https://open.* URL");
  }

  const runtimeUrl = cleanEnvValue(env.DEEPSEEK_RUNTIME_URL || "http://127.0.0.1:7878");
  try {
    const parsed = new URL(runtimeUrl);
    const localHosts = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);
    if (!["http:", "https:"].includes(parsed.protocol)) {
      add(errors, "invalid_runtime_url", "DEEPSEEK_RUNTIME_URL must use http or https");
    }
    if (!localHosts.has(parsed.hostname)) {
      add(errors, "remote_runtime_url", "DEEPSEEK_RUNTIME_URL must point at localhost on Lighthouse");
    }
  } catch {
    add(errors, "invalid_runtime_url", "DEEPSEEK_RUNTIME_URL is not a valid URL");
  }

  const workspace = cleanEnvValue(env.DEEPSEEK_WORKSPACE);
  if (workspace && !workspace.startsWith("/")) {
    add(errors, "relative_workspace", "DEEPSEEK_WORKSPACE must be an absolute path");
  }
  if (
    workspace &&
    workspaceRoot &&
    workspace !== workspaceRoot &&
    !workspace.startsWith(`${workspaceRoot}/`)
  ) {
    add(warnings, "workspace_root", `DEEPSEEK_WORKSPACE is outside ${workspaceRoot}`);
  }

  const threadMapPath = cleanEnvValue(env.FEISHU_THREAD_MAP_PATH);
  if (threadMapPath && !threadMapPath.startsWith("/")) {
    add(errors, "relative_thread_map", "FEISHU_THREAD_MAP_PATH must be an absolute path");
  }

  const allowGroups = parseBool(env.FEISHU_ALLOW_GROUPS, false);
  const requirePrefix = parseBool(env.FEISHU_REQUIRE_PREFIX_IN_GROUP, true);
  const allowUnlisted = parseBool(env.DEEPSEEK_ALLOW_UNLISTED, false);
  const allowlist = parseList(env.DEEPSEEK_CHAT_ALLOWLIST);

  if (!allowlist.length && allowUnlisted) {
    add(warnings, "pairing_mode_open", "DEEPSEEK_ALLOW_UNLISTED=true leaves first-pairing mode open");
  } else if (!allowlist.length) {
    add(warnings, "not_paired", "DEEPSEEK_CHAT_ALLOWLIST is empty; all chats will be refused");
  }
  if (allowGroups && allowUnlisted) {
    add(errors, "open_group_control", "Group control cannot be enabled while unlisted chats are allowed");
  }
  if (allowGroups && !requirePrefix) {
    add(warnings, "group_without_prefix", "Group control is enabled without requiring FEISHU_GROUP_PREFIX");
  }
  if (!allowGroups) {
    add(info, "dm_only", "Direct-message control is enabled; group chats are disabled");
  }

  const maxReplyChars = Number(env.FEISHU_MAX_REPLY_CHARS || 3500);
  if (!Number.isFinite(maxReplyChars) || maxReplyChars < 100) {
    add(errors, "invalid_max_reply_chars", "FEISHU_MAX_REPLY_CHARS must be at least 100");
  }
  const turnTimeoutMs = Number(env.DEEPSEEK_TURN_TIMEOUT_MS || 900000);
  if (!Number.isFinite(turnTimeoutMs) || turnTimeoutMs < 1000) {
    add(errors, "invalid_turn_timeout", "DEEPSEEK_TURN_TIMEOUT_MS must be at least 1000");
  }

  if (runtimeEnv) {
    const runtimeToken = cleanEnvValue(runtimeEnv.DEEPSEEK_RUNTIME_TOKEN);
    const bridgeToken = cleanEnvValue(env.DEEPSEEK_RUNTIME_TOKEN);
    if (!runtimeToken) {
      add(errors, "missing_runtime_token", "runtime.env is missing DEEPSEEK_RUNTIME_TOKEN");
    } else if (isPlaceholderValue(runtimeToken)) {
      add(errors, "placeholder_runtime_token", "runtime.env DEEPSEEK_RUNTIME_TOKEN is still a placeholder");
    } else if (bridgeToken && bridgeToken !== runtimeToken) {
      add(errors, "token_mismatch", "Runtime and bridge DEEPSEEK_RUNTIME_TOKEN values do not match");
    }

    const apiKey = cleanEnvValue(runtimeEnv.DEEPSEEK_API_KEY);
    if (!apiKey) {
      add(warnings, "missing_api_key", "runtime.env is missing DEEPSEEK_API_KEY");
    } else if (isPlaceholderValue(apiKey)) {
      add(warnings, "placeholder_api_key", "runtime.env DEEPSEEK_API_KEY is still a placeholder");
    }

    const runtimePort = Number(runtimeEnv.DEEPSEEK_RUNTIME_PORT || 7878);
    if (!Number.isInteger(runtimePort) || runtimePort <= 0 || runtimePort > 65535) {
      add(errors, "invalid_runtime_port", "DEEPSEEK_RUNTIME_PORT must be a valid TCP port");
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
  const lines = ["Feishu bridge config validation"];
  for (const item of result.errors) lines.push(`[fail] ${item.message}`);
  for (const item of result.warnings) lines.push(`[warn] ${item.message}`);
  for (const item of result.info) lines.push(`[info] ${item.message}`);
  if (result.ok) lines.push("[ok] No blocking config errors found");
  return lines.join("\n");
}

export function helpText() {
  return [
    "DeepSeek phone bridge commands:",
    "/help - show this help",
    "/status - runtime and workspace status",
    "/threads - recent runtime threads",
    "/new - create a new thread for this chat",
    "/resume <thread_id> - bind this chat to an existing thread",
    "/interrupt - interrupt the active turn",
    "/compact - compact the current thread",
    "/allow <approval_id> [remember] - approve a pending tool call",
    "/deny <approval_id> - deny a pending tool call",
    "",
    "Anything else is sent as a DeepSeek prompt."
  ].join("\n");
}
