import fs from "node:fs/promises";
import path from "node:path";

import {
  activeTurnBlock,
  activeTurnKeyboard,
  approvalKeyboard,
  callbackAction,
  commandAction,
  compactRuntimeError,
  controlKeyboard,
  envFirst,
  helpText,
  isAllowed,
  isGroupChat,
  latestRunningTurn,
  looksLikePollingConflict,
  pairingRefusalText,
  parseBool,
  parseCommand,
  parseList,
  preservedChatStateFields,
  splitMessage,
  stripGroupPrefix,
  threadListKeyboard,
  telegramIdentity,
  telegramRetryDelayMs
} from "./lib.mjs";

class ThreadStore {
  static async open(filePath) {
    const store = new ThreadStore(filePath);
    await store.load();
    return store;
  }

  constructor(filePath) {
    this.filePath = filePath;
    this.data = { chats: {}, messages: [], actions: {} };
  }

  async load() {
    try {
      const raw = await fs.readFile(this.filePath, "utf8");
      this.data = JSON.parse(raw);
      if (!this.data.chats) this.data.chats = {};
      if (!Array.isArray(this.data.messages)) this.data.messages = [];
      if (!this.data.actions || typeof this.data.actions !== "object") this.data.actions = {};
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }

  async recordMessage(messageKey) {
    if (!messageKey) return false;
    if (!Array.isArray(this.data.messages)) this.data.messages = [];
    if (this.data.messages.includes(messageKey)) return true;
    this.data.messages.push(messageKey);
    this.data.messages = this.data.messages.slice(-500);
    await this.save();
    return false;
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
    if (!this.data.actions || typeof this.data.actions !== "object") this.data.actions = {};
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
    if (!token || !this.data.actions) return null;
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
    const entries = Object.entries(this.data.actions || {});
    const cutoff = Date.now() - 24 * 60 * 60 * 1000;
    const fresh = entries.filter(([, action]) => {
      const time = Date.parse(action.createdAt || "");
      return Number.isFinite(time) && time >= cutoff;
    });
    this.data.actions = Object.fromEntries(fresh.slice(-200));
  }

  async save() {
    const dir = path.dirname(this.filePath);
    await fs.mkdir(dir, { recursive: true, mode: 0o700 });
    const tmp = `${this.filePath}.tmp`;
    await fs.writeFile(tmp, `${JSON.stringify(this.data, null, 2)}\n`, { mode: 0o600 });
    await fs.rename(tmp, this.filePath);
  }
}

const config = {
  botToken: requiredEnv("TELEGRAM_BOT_TOKEN"),
  apiBase: (process.env.TELEGRAM_API_BASE || "https://api.telegram.org").replace(/\/+$/, ""),
  runtimeUrl: (envFirst(process.env, "CODEWHALE_RUNTIME_URL", "DEEPSEEK_RUNTIME_URL") || "http://127.0.0.1:7878").replace(/\/+$/, ""),
  runtimeToken: requiredEnvFirst("CODEWHALE_RUNTIME_TOKEN", "DEEPSEEK_RUNTIME_TOKEN"),
  workspace: envFirst(process.env, "CODEWHALE_WORKSPACE", "DEEPSEEK_WORKSPACE") || process.cwd(),
  model: envFirst(process.env, "CODEWHALE_MODEL", "DEEPSEEK_MODEL") || "auto",
  mode: envFirst(process.env, "CODEWHALE_MODE", "DEEPSEEK_MODE") || "agent",
  allowShell: parseBool(envFirst(process.env, "CODEWHALE_ALLOW_SHELL", "DEEPSEEK_ALLOW_SHELL"), true),
  trustMode: parseBool(envFirst(process.env, "CODEWHALE_TRUST_MODE", "DEEPSEEK_TRUST_MODE"), false),
  autoApprove: parseBool(envFirst(process.env, "CODEWHALE_AUTO_APPROVE", "DEEPSEEK_AUTO_APPROVE"), false),
  allowlist: parseList(
    envFirst(process.env, "TELEGRAM_CHAT_ALLOWLIST", "CODEWHALE_CHAT_ALLOWLIST", "DEEPSEEK_CHAT_ALLOWLIST")
  ),
  allowUnlisted: parseBool(
    envFirst(process.env, "TELEGRAM_ALLOW_UNLISTED", "CODEWHALE_ALLOW_UNLISTED", "DEEPSEEK_ALLOW_UNLISTED"),
    false
  ),
  threadMapPath:
    process.env.TELEGRAM_THREAD_MAP_PATH ||
    "/var/lib/codewhale-telegram-bridge/thread-map.json",
  allowGroups: parseBool(process.env.TELEGRAM_ALLOW_GROUPS, false),
  requirePrefixInGroup: parseBool(process.env.TELEGRAM_REQUIRE_PREFIX_IN_GROUP, true),
  groupPrefix: process.env.TELEGRAM_GROUP_PREFIX || "/cw",
  maxReplyChars: Math.min(Number(process.env.TELEGRAM_MAX_REPLY_CHARS || 3500), 4096),
  pollTimeoutSeconds: Number(process.env.TELEGRAM_POLL_TIMEOUT_SECONDS || 50),
  turnTimeoutMs: Number(envFirst(process.env, "CODEWHALE_TURN_TIMEOUT_MS", "DEEPSEEK_TURN_TIMEOUT_MS") || 900000)
};

const threadStore = await ThreadStore.open(config.threadMapPath);
let stopping = false;
let updateOffset = Number(process.env.TELEGRAM_UPDATE_OFFSET || 0);

process.once("SIGINT", () => {
  stopping = true;
});
process.once("SIGTERM", () => {
  stopping = true;
});

console.log("Starting CodeWhale Telegram bridge");
console.log(`Runtime: ${config.runtimeUrl}`);
console.log(`Workspace: ${config.workspace}`);
if (!config.allowlist.length && !config.allowUnlisted) {
  console.log("No allowlist configured. Incoming chats will receive their IDs and be refused.");
}

await configureBotCommands().catch((error) => {
  console.error("failed to configure Telegram bot command menu", error);
});
void reattachActiveTurns().catch((error) => {
  console.error("failed to reattach active Telegram bridge turns", error);
});
await pollTelegram();

async function configureBotCommands() {
  await telegramApi("setMyCommands", {
    commands: [
      { command: "menu", description: "Open CodeWhale controls" },
      { command: "status", description: "Show runtime and workspace status" },
      { command: "threads", description: "List recent runtime threads" },
      { command: "new", description: "Create a new thread" },
      { command: "interrupt", description: "Interrupt the active turn" },
      { command: "compact", description: "Compact the current thread" },
      { command: "help", description: "Show command help" }
    ]
  });
}

async function pollTelegram() {
  while (!stopping) {
    try {
      const updates = await telegramApi("getUpdates", {
        offset: updateOffset || undefined,
        timeout: config.pollTimeoutSeconds,
        allowed_updates: ["message", "callback_query"]
      });
      for (const update of updates || []) {
        if (update.update_id != null) updateOffset = Math.max(updateOffset, update.update_id + 1);
        await handleIncomingUpdate(update).catch((error) => {
          console.error("failed to handle incoming Telegram update", error);
        });
      }
    } catch (error) {
      if (looksLikePollingConflict(error)) {
        console.warn("Telegram getUpdates conflict; another bridge is polling this bot. Retrying in 10s.");
        await delay(10000);
        continue;
      }
      const waitMs = telegramRetryDelayMs(error);
      console.error(`Telegram poll failed: ${error.message}. Retrying in ${Math.round(waitMs / 1000)}s.`);
      await delay(waitMs);
    }
  }
}

async function handleIncomingUpdate(update) {
  if (update.callback_query) {
    await handleCallbackQuery(update.callback_query);
    return;
  }

  const identity = telegramIdentity(update);
  if (!identity.chatId || !identity.messageId) return;
  if (identity.isBot) return;

  const messageKey = `${identity.chatId}:${identity.messageId}`;
  if (await threadStore.recordMessage(messageKey)) return;

  if (!identity.text) {
    await sendText(identity.chatId, "Only text messages are supported in this first bridge.");
    return;
  }

  const scoped = stripGroupPrefix(identity.text, {
    chatType: identity.chatType,
    requirePrefix: config.requirePrefixInGroup,
    prefix: config.groupPrefix
  });
  if (!scoped.accepted) return;

  if (isGroupChat(identity.chatType) && !config.allowGroups) {
    await sendText(
      identity.chatId,
      "Group chat control is disabled for this bridge. DM the bot, or set TELEGRAM_ALLOW_GROUPS=true and allowlist this chat."
    );
    return;
  }

  if (!isAllowed(identity, config.allowlist, config.allowUnlisted)) {
    await sendText(identity.chatId, pairingRefusalText(identity));
    return;
  }

  const command = parseCommand(scoped.text);
  await handleCommand(identity.chatId, command);
}

async function handleCommand(chatId, command) {
  const action = commandAction(command);
  switch (action.kind) {
    case "help":
      await sendText(chatId, helpText(), { replyMarkup: controlKeyboard() });
      return;
    case "menu":
      await sendMenu(chatId);
      return;
    case "status":
      await sendStatus(chatId);
      return;
    case "threads":
      await sendThreads(chatId);
      return;
    case "new_thread": {
      const state = await ensureThread(chatId, { forceNew: true });
      await sendText(chatId, `Created thread ${state.threadId}`, { replyMarkup: controlKeyboard() });
      return;
    }
    case "resume":
      await resumeThread(chatId, action.threadId);
      return;
    case "interrupt":
      await interruptActiveTurn(chatId);
      return;
    case "compact":
      await compactThread(chatId);
      return;
    case "approval":
      await decideApproval(chatId, action);
      return;
    case "set_model":
      await setChatModel(chatId, action.modelName);
      return;
    case "prompt":
      await runPrompt(chatId, action.prompt);
      return;
    default:
      await sendText(chatId, helpText(), { replyMarkup: controlKeyboard() });
  }
}

async function handleCallbackQuery(query) {
  const chat = query.message?.chat || {};
  const from = query.from || {};
  const identity = {
    chatId: chat.id != null ? String(chat.id) : "",
    messageId: query.message?.message_id != null ? String(query.message.message_id) : "",
    chatType: chat.type || "",
    userId: from.id != null ? String(from.id) : "",
    username: from.username ? `@${from.username}` : "",
    firstName: from.first_name || "",
    isBot: Boolean(from.is_bot)
  };

  if (!identity.chatId || !query.id) return;
  if (identity.isBot) return;

  if (isGroupChat(identity.chatType) && !config.allowGroups) {
    await answerCallback(query.id, "Group control is disabled.");
    return;
  }
  if (!isAllowed(identity, config.allowlist, config.allowUnlisted)) {
    await answerCallback(query.id, "This chat is not allowlisted.");
    return;
  }

  const action = callbackAction(query.data);
  if (!action) {
    await answerCallback(query.id, "Unknown action.");
    return;
  }

  await answerCallback(query.id, "Working...");
  await handleModalAction(identity.chatId, action, query);
}

async function handleModalAction(chatId, action, query = null) {
  switch (action.kind) {
    case "help":
      await sendText(chatId, helpText(), { replyMarkup: controlKeyboard() });
      return;
    case "status":
      await sendStatus(chatId);
      return;
    case "threads":
      await sendThreads(chatId);
      return;
    case "new_thread": {
      const state = await ensureThread(chatId, { forceNew: true });
      await sendText(chatId, `Created thread ${state.threadId}`, { replyMarkup: controlKeyboard() });
      return;
    }
    case "interrupt":
      await interruptActiveTurn(chatId);
      return;
    case "compact":
      await compactThread(chatId);
      return;
    case "set_model":
      await setChatModel(chatId, action.modelName);
      return;
    case "stored_action":
      await handleStoredAction(chatId, action, query);
      return;
    default:
      await sendText(chatId, helpText(), { replyMarkup: controlKeyboard() });
  }
}

async function handleStoredAction(chatId, action, query = null) {
  const stored = await threadStore.getAction(action.token);
  if (!stored) {
    await sendText(chatId, "That action expired. Open /menu and try again.");
    return;
  }

  if (stored.kind === "resume") {
    await resumeThread(chatId, stored.threadId);
    return;
  }

  if (stored.kind === "approval") {
    const suffix = action.suffix || "";
    const decision = suffix === "deny" ? "deny" : "allow";
    const remember = suffix === "remember";
    await threadStore.takeAction(action.token);
    await decideApproval(chatId, {
      decision,
      approvalId: stored.approvalId,
      remember
    });
    if (query?.message?.message_id) {
      await editMessageReplyMarkup(chatId, query.message.message_id, null).catch(() => {});
    }
    return;
  }

  await sendText(chatId, "That action is no longer supported.");
}

async function sendMenu(chatId) {
  const state = await threadStore.getChat(chatId);
  await sendText(
    chatId,
    [
      "CodeWhale controls",
      state?.threadId ? `thread=${state.threadId}` : "thread=(new on first prompt)",
      `model=${state?.model || config.model}`
    ].join("\n"),
    { replyMarkup: controlKeyboard() }
  );
}

async function ensureThread(chatId, { forceNew = false } = {}) {
  const existing = await threadStore.getChat(chatId);
  if (existing?.threadId && !forceNew) return existing;

  const effectiveModel = existing?.model || config.model;
  const thread = await runtimeJson("/v1/threads", {
    method: "POST",
    body: {
      model: effectiveModel,
      workspace: config.workspace,
      mode: config.mode,
      allow_shell: config.allowShell,
      trust_mode: config.trustMode,
      auto_approve: config.autoApprove,
      archived: false,
      system_prompt:
        "You are being controlled from a Telegram phone chat. Keep status updates concise. Ask for tool approvals when needed; do not assume mobile messages imply blanket approval."
    }
  });

  const state = {
    ...preservedChatStateFields(existing),
    threadId: thread.id,
    lastSeq: 0,
    activeTurnId: null,
    updatedAt: new Date().toISOString()
  };
  await threadStore.setChat(chatId, state);
  return state;
}

async function runPrompt(chatId, prompt) {
  if (!prompt.trim()) {
    await sendText(chatId, helpText(), { replyMarkup: controlKeyboard() });
    return;
  }
  const state = await ensureThread(chatId);
  const effectiveModel = state?.model || config.model;
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}`);
  const activeBlock = activeTurnBlock(detail, state);
  if (activeBlock) {
    await threadStore.patchChat(chatId, {
      activeTurnId: activeBlock.turnId,
      updatedAt: new Date().toISOString()
    });
    await sendText(chatId, activeBlock.message, { replyMarkup: activeTurnKeyboard() });
    return;
  }
  if (state.activeTurnId) {
    await threadStore.patchChat(chatId, { activeTurnId: null });
  }
  const sinceSeq = Number(detail.latest_seq || state.lastSeq || 0);

  const turnResponse = await runtimeJson(
    `/v1/threads/${encodeURIComponent(state.threadId)}/turns`,
    {
      method: "POST",
      body: {
        prompt,
        input_summary: prompt.slice(0, 200),
        model: effectiveModel,
        mode: config.mode,
        allow_shell: config.allowShell,
        trust_mode: config.trustMode,
        auto_approve: config.autoApprove
      }
    }
  );

  const turnId = turnResponse.turn?.id;
  await threadStore.patchChat(chatId, {
    activeTurnId: turnId || null,
    lastSeq: sinceSeq,
    updatedAt: new Date().toISOString()
  });
  await sendText(chatId, `Started turn ${turnId || "(unknown)"}`, {
    replyMarkup: activeTurnKeyboard()
  });

  try {
    await streamTurnEvents(chatId, state.threadId, turnId, sinceSeq);
  } finally {
    await threadStore.patchChat(chatId, {
      activeTurnId: null,
      updatedAt: new Date().toISOString()
    });
  }
}

async function reattachActiveTurns() {
  for (const [chatId, state] of threadStore.listChats()) {
    if (!state?.threadId || !state.activeTurnId) continue;

    const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}`);
    const runningTurn = latestRunningTurn(detail);
    if (!runningTurn) {
      await threadStore.patchChat(chatId, {
        activeTurnId: null,
        lastSeq: Number(detail.latest_seq || state.lastSeq || 0),
        updatedAt: new Date().toISOString()
      });
      await sendText(chatId, `Bridge restarted. No active turn remains for ${state.threadId}.`);
      continue;
    }

    const turnId = runningTurn.id || state.activeTurnId;
    const sinceSeq = Number(state.lastSeq || 0);
    await threadStore.patchChat(chatId, {
      activeTurnId: turnId,
      updatedAt: new Date().toISOString()
    });
    await sendText(
      chatId,
      `Bridge restarted. Reattaching to active turn ${turnId} from seq ${sinceSeq}.`
    );
    try {
      await streamTurnEvents(chatId, state.threadId, turnId, sinceSeq);
    } finally {
      await threadStore.patchChat(chatId, {
        activeTurnId: null,
        updatedAt: new Date().toISOString()
      });
    }
  }
}

async function streamTurnEvents(chatId, threadId, turnId, sinceSeq) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), config.turnTimeoutMs);
  let responseText = "";
  let latestSeq = sinceSeq;
  let sentProgressAt = Date.now();

  try {
    const response = await fetch(
      `${config.runtimeUrl}/v1/threads/${encodeURIComponent(threadId)}/events?since_seq=${sinceSeq}`,
      {
        headers: authHeaders(),
        signal: controller.signal
      }
    );
    if (!response.ok) {
      const body = await readJsonSafe(response);
      throw new Error(compactRuntimeError(response.status, body));
    }

    for await (const event of readSse(response)) {
      if (!event.data) continue;
      const record = JSON.parse(event.data);
      latestSeq = Math.max(latestSeq, Number(record.seq || 0));
      await threadStore.patchChat(chatId, { lastSeq: latestSeq });

      if (turnId && record.turn_id && record.turn_id !== turnId) continue;

      if (record.event === "item.delta" && record.payload?.kind === "agent_message") {
        responseText += record.payload.delta || "";
        const now = Date.now();
        if (responseText.length > config.maxReplyChars && now - sentProgressAt > 15000) {
          await sendText(chatId, responseText.slice(0, config.maxReplyChars));
          responseText = responseText.slice(config.maxReplyChars);
          sentProgressAt = now;
        }
      }

      if (record.event === "approval.required") {
        const approval = record.payload || {};
        const approvalId = approval.approval_id || approval.id;
        if (!approvalId) {
          await sendText(
            chatId,
            [
              "Approval required",
              `tool=${approval.tool_name || "unknown"}`,
              approval.description || "",
              "",
              "No approval_id was provided by the runtime; use /status and retry from the TUI."
            ]
              .filter(Boolean)
              .join("\n"),
            { replyMarkup: controlKeyboard() }
          );
          continue;
        }
        const actionToken = await threadStore.putAction({
          kind: "approval",
          approvalId
        });
        await sendText(
          chatId,
          [
            "Approval required",
            `tool=${approval.tool_name || "unknown"}`,
            `approval_id=${approvalId}`,
            approval.description || "",
            "",
            `Tap a button, or reply /allow ${approvalId}`,
            `Reply /deny ${approvalId}`
          ]
            .filter(Boolean)
            .join("\n"),
          { replyMarkup: approvalKeyboard(actionToken) }
        );
      }

      if (record.event === "turn.completed") {
        const turn = record.payload?.turn || {};
        const status = turn.status || "completed";
        const error = turn.error ? `\n${turn.error}` : "";
        if (status !== "completed") {
          await sendText(chatId, `Turn ${status}.${error}`.trim(), {
            replyMarkup: controlKeyboard()
          });
        } else {
          await sendText(chatId, responseText.trim() || "Turn completed.", {
            replyMarkup: controlKeyboard()
          });
        }
        return;
      }

      if (record.event === "turn.lifecycle") {
        const status = record.payload?.turn?.status || record.payload?.status;
        if (["failed", "canceled", "interrupted"].includes(status)) {
      await sendText(chatId, `Turn ${status}.`, { replyMarkup: controlKeyboard() });
          return;
        }
      }
    }
  } catch (error) {
    if (error.name === "AbortError") {
      await sendText(chatId, `Turn timed out after ${Math.round(config.turnTimeoutMs / 1000)}s.`);
      return;
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function sendStatus(chatId) {
  const [health, runtimeInfo, workspace] = await Promise.all([
    runtimeJson("/health", { auth: false }),
    runtimeJson("/v1/runtime/info"),
    runtimeJson("/v1/workspace/status")
  ]);
  await sendText(
    chatId,
    [
      `runtime=${health.status || "unknown"}`,
      `version=${runtimeInfo.version || "unknown"}`,
      `bind=${runtimeInfo.bind_host}:${runtimeInfo.port}`,
      `auth_required=${runtimeInfo.auth_required}`,
      `workspace=${workspace.workspace}`,
      `git_repo=${workspace.git_repo}`,
      workspace.branch ? `branch=${workspace.branch}` : "",
      `staged=${workspace.staged} unstaged=${workspace.unstaged} untracked=${workspace.untracked}`
    ]
      .filter(Boolean)
      .join("\n"),
    { replyMarkup: controlKeyboard() }
  );
}

async function sendThreads(chatId) {
  const threads = await runtimeJson("/v1/threads/summary?limit=8&include_archived=true");
  if (!threads.length) {
    await sendText(chatId, "No runtime threads yet.", { replyMarkup: controlKeyboard() });
    return;
  }
  const actions = [];
  for (const [index, thread] of threads.slice(0, 8).entries()) {
    const token = await threadStore.putAction({
      kind: "resume",
      threadId: thread.id
    });
    actions.push({ token, label: `Resume ${index + 1}` });
  }
  await sendText(
    chatId,
    threads
      .map((thread, index) => {
        const status = thread.latest_turn_status || "none";
        return `${index + 1}. ${thread.id} [${status}] ${thread.title || thread.preview || ""}`;
      })
      .join("\n"),
    { replyMarkup: threadListKeyboard(actions) }
  );
}

async function resumeThread(chatId, args) {
  const threadId = args.trim();
  if (!threadId) {
    await sendText(chatId, "Usage: /resume <thread_id>");
    return;
  }
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(threadId)}`);
  const existing = await threadStore.getChat(chatId);
  await threadStore.setChat(chatId, {
    ...preservedChatStateFields(existing),
    threadId,
    lastSeq: Number(detail.latest_seq || 0),
    activeTurnId: null,
    updatedAt: new Date().toISOString()
  });
  await sendText(chatId, `Resumed thread ${threadId}`, { replyMarkup: controlKeyboard() });
}

async function interruptActiveTurn(chatId) {
  const state = await threadStore.getChat(chatId);
  if (!state?.threadId) {
    await sendText(chatId, "No runtime thread recorded for this chat.");
    return;
  }
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}`);
  const runningTurn = latestRunningTurn(detail);
  const turnId = state.activeTurnId || runningTurn?.id;
  if (!turnId) {
    await sendText(chatId, "No active turn recorded for this chat.");
    return;
  }
  await runtimeJson(
    `/v1/threads/${encodeURIComponent(state.threadId)}/turns/${encodeURIComponent(
      turnId
    )}/interrupt`,
    { method: "POST" }
  );
  await threadStore.patchChat(chatId, {
    activeTurnId: turnId,
    updatedAt: new Date().toISOString()
  });
  await sendText(chatId, `Interrupt requested for ${turnId}`, { replyMarkup: controlKeyboard() });
}

async function compactThread(chatId) {
  const state = await ensureThread(chatId);
  const result = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}/compact`, {
    method: "POST",
    body: { reason: "telegram bridge request" }
  });
  await sendText(chatId, `Compaction started: ${result.turn?.id || "unknown turn"}`, {
    replyMarkup: activeTurnKeyboard()
  });
}

async function decideApproval(chatId, action) {
  const decision = action.decision;
  const { approvalId, remember } = action;
  if (!approvalId) {
    await sendText(
      chatId,
      `Usage: /${decision} <approval_id>${decision === "allow" ? " [remember]" : ""}`
    );
    return;
  }
  await runtimeJson(`/v1/approvals/${encodeURIComponent(approvalId)}`, {
    method: "POST",
    body: { decision, remember }
  });
  await sendText(chatId, `Approval ${approvalId}: ${decision}${remember ? " and remember" : ""}`);
}

async function setChatModel(chatId, modelName) {
  if (!modelName || modelName === "default") {
    await threadStore.patchChat(chatId, {
      model: null,
      updatedAt: new Date().toISOString()
    });
    await sendText(chatId, `Reset per-chat model. Using bridge default: ${config.model}`, {
      replyMarkup: controlKeyboard()
    });
    return;
  }
  await threadStore.patchChat(chatId, {
    model: modelName,
    updatedAt: new Date().toISOString()
  });
  await sendText(chatId, `Per-chat model set to: ${modelName}`, { replyMarkup: controlKeyboard() });
}

async function sendText(chatId, text, options = {}) {
  const chunks = splitMessage(text, config.maxReplyChars);
  for (const [index, chunk] of chunks.entries()) {
    const body = {
      chat_id: chatId,
      text: chunk,
      disable_web_page_preview: true
    };
    if (options.replyMarkup && index === chunks.length - 1) {
      body.reply_markup = options.replyMarkup;
    }
    await telegramApi("sendMessage", body);
  }
}

async function answerCallback(callbackQueryId, text = "") {
  await telegramApi("answerCallbackQuery", {
    callback_query_id: callbackQueryId,
    text: text.slice(0, 200),
    show_alert: false
  });
}

async function editMessageReplyMarkup(chatId, messageId, replyMarkup) {
  await telegramApi("editMessageReplyMarkup", {
    chat_id: chatId,
    message_id: messageId,
    reply_markup: replyMarkup
  });
}

async function telegramApi(method, body = {}) {
  const response = await fetch(`${config.apiBase}/bot${config.botToken}/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body)
  });
  const payload = await readJsonSafe(response);
  if (!response.ok || payload?.ok === false) {
    const error = new Error(
      payload?.description || `Telegram API request failed (${response.status})`
    );
    error.errorCode = payload?.error_code || response.status;
    error.description = payload?.description || "";
    error.parameters = payload?.parameters || {};
    throw error;
  }
  return payload.result;
}

async function runtimeJson(route, options = {}) {
  const response = await fetch(`${config.runtimeUrl}${route}`, {
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

function authHeaders() {
  return { authorization: `Bearer ${config.runtimeToken}` };
}

async function readJsonSafe(response) {
  const text = await response.text();
  if (!text) return {};
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

async function* readSse(response) {
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

function requiredEnv(name) {
  const value = process.env[name];
  if (!value || !value.trim()) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

function requiredEnvFirst(...names) {
  const value = envFirst(process.env, ...names);
  if (!value) {
    throw new Error(`${names.join(" or ")} is required`);
  }
  return value;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
