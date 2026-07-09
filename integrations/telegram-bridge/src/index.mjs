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
  isTelegramMarkdownParseError,
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
  telegramMessageBody,
  telegramPollingConflictDelayMs,
  telegramRetryDelayMs,
  telegramSendRetryDelayMs
} from "./lib.mjs";
import {
  createRuntimeClient,
  readJsonSafe,
  readSse,
  ThreadStore as CoreThreadStore
} from "../../bridge-core/src/lib.mjs";

const TYPING_INTERVAL_MS = 2000;
const TYPING_TIMEOUT_MS = 1500;
const LAST_SEQ_FLUSH_INTERVAL_MS = 2000;

class ThreadStore extends CoreThreadStore {
  constructor(filePath) {
    super(filePath, { messageLimit: 500, actions: true });
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

const { runtimeJson, authHeaders } = createRuntimeClient(config);

const threadStore = await ThreadStore.open(config.threadMapPath);
const activeTurnTasks = new Map();
let stopping = false;
let updateOffset = threadStore.getCursor(
  "telegram.update_offset",
  Number(process.env.TELEGRAM_UPDATE_OFFSET || 0)
);

function requestStop() {
  stopping = true;
  abortActiveTurnStreams();
}

process.once("SIGINT", requestStop);
process.once("SIGTERM", requestStop);

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
  let pollingConflictAttempts = 0;
  while (!stopping) {
    try {
      const updates = await telegramApi("getUpdates", {
        offset: updateOffset || undefined,
        timeout: config.pollTimeoutSeconds,
        allowed_updates: ["message", "callback_query"]
      });
      pollingConflictAttempts = 0;
      for (const update of updates || []) {
        try {
          await handleIncomingUpdate(update);
          await markUpdateHandled(update);
        } catch (error) {
          console.error("failed to handle incoming Telegram update", error);
          break;
        }
      }
    } catch (error) {
      if (looksLikePollingConflict(error)) {
        const waitMs = telegramPollingConflictDelayMs(pollingConflictAttempts);
        pollingConflictAttempts += 1;
        if (waitMs == null) {
          throw new Error(
            "Telegram getUpdates conflict; another bridge is polling this bot token. Stop the other bridge process or use a different token."
          );
        }
        console.warn(
          `Telegram getUpdates conflict; another bridge is polling this bot. Retrying in ${Math.round(waitMs / 1000)}s.`
        );
        await delay(waitMs);
        continue;
      }
      pollingConflictAttempts = 0;
      const waitMs = telegramRetryDelayMs(error);
      console.error(`Telegram poll failed: ${error.message}. Retrying in ${Math.round(waitMs / 1000)}s.`);
      await delay(waitMs);
    }
  }
}

async function markUpdateHandled(update) {
  if (update.update_id == null) return;
  if (!Number.isFinite(Number(update.update_id))) return;
  const nextOffset = Math.max(updateOffset, Number(update.update_id) + 1);
  if (nextOffset === updateOffset) return;
  updateOffset = nextOffset;
  await threadStore.setCursor("telegram.update_offset", updateOffset);
}

async function handleIncomingUpdate(update) {
  if (update.callback_query) {
    if (await isReplayCallbackUpdate(update)) return;
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

async function isReplayCallbackUpdate(update) {
  if (update.update_id == null) return false;
  return threadStore.recordMessage(`callback:${update.update_id}`);
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
      startPromptTurn(chatId, action.prompt);
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

  answerCallback(query.id, "Working...").catch((error) => {
    console.warn("failed to acknowledge Telegram callback", error);
  });
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
    await threadStore.takeAction(action.token);
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

function startPromptTurn(chatId, prompt) {
  if (activeTurnTasks.has(chatId)) {
    void sendText(chatId, "Thread already has an active turn. Wait for it to finish or send /interrupt.", {
      replyMarkup: activeTurnKeyboard()
    }).catch((error) => {
      console.error("failed to report active Telegram bridge turn", error);
    });
    return;
  }

  const controller = new AbortController();
  const task = { controller };
  activeTurnTasks.set(chatId, task);
  void runPrompt(chatId, prompt, { signal: controller.signal })
    .catch((error) => {
      console.error("failed to run Telegram bridge prompt", error);
    })
    .finally(() => {
      if (activeTurnTasks.get(chatId) === task) {
        activeTurnTasks.delete(chatId);
      }
    });
}

function abortActiveTurnStreams() {
  for (const task of activeTurnTasks.values()) {
    task.controller?.abort();
  }
}

async function clearActiveTurn(chatId) {
  await threadStore.patchChat(chatId, {
    activeTurnId: null,
    updatedAt: new Date().toISOString()
  }).catch((error) => {
    console.error("failed to clear Telegram bridge active turn", error);
  });
}

function startTrackedTurnStream(chatId, threadId, turnId, sinceSeq) {
  if (activeTurnTasks.has(chatId)) return false;

  const controller = new AbortController();
  const task = { controller };
  activeTurnTasks.set(chatId, task);
  void streamTurnEvents(chatId, threadId, turnId, sinceSeq, { signal: controller.signal })
    .catch((error) => {
      console.error("failed to stream Telegram bridge turn", error);
    })
    .finally(async () => {
      if (activeTurnTasks.get(chatId) === task) {
        activeTurnTasks.delete(chatId);
      }
      if (!stopping) {
        await clearActiveTurn(chatId);
      }
    });
  return true;
}

async function runPrompt(chatId, prompt, options = {}) {
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
  await sendTurnText(chatId, `Started turn ${turnId || "(unknown)"}`, {
    replyMarkup: activeTurnKeyboard()
  });

  try {
    await streamTurnEvents(chatId, state.threadId, turnId, sinceSeq, options);
  } finally {
    if (!stopping) {
      await clearActiveTurn(chatId);
    }
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
    await sendTurnText(
      chatId,
      `Bridge restarted. Reattaching to active turn ${turnId} from seq ${sinceSeq}.`
    );
    startTrackedTurnStream(chatId, state.threadId, turnId, sinceSeq);
  }
}

async function streamTurnEvents(chatId, threadId, turnId, sinceSeq, options = {}) {
  const controller = new AbortController();
  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, config.turnTimeoutMs);
  const abortFromCaller = () => controller.abort();
  if (options.signal?.aborted) {
    controller.abort();
  } else {
    options.signal?.addEventListener("abort", abortFromCaller, { once: true });
  }
  let responseText = "";
  let latestSeq = sinceSeq;
  let flushedSeq = sinceSeq;
  let lastSeqFlushAt = 0;
  let sentProgressAt = Date.now();
  let typingPaused = false;
  let typingInFlight = false;

  async function flushLastSeq(force = false) {
    if (latestSeq <= flushedSeq) return;
    if (!force && Date.now() - lastSeqFlushAt < LAST_SEQ_FLUSH_INTERVAL_MS) return;
    await threadStore.patchChat(chatId, { lastSeq: latestSeq });
    flushedSeq = latestSeq;
    lastSeqFlushAt = Date.now();
  }

  const tickTyping = async () => {
    if (stopping || typingPaused || typingInFlight) return;
    typingInFlight = true;
    try {
      await sendTypingAction(chatId);
    } catch (error) {
      console.warn("failed to send Telegram typing action", error);
    } finally {
      typingInFlight = false;
    }
  };
  const typingTimer = setInterval(() => {
    void tickTyping();
  }, TYPING_INTERVAL_MS);
  typingTimer.unref?.();

  try {
    void tickTyping();
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
      await flushLastSeq(false);

      if (turnId && record.turn_id && record.turn_id !== turnId) continue;
      const lifecycleStatus =
        record.event === "turn.lifecycle"
          ? record.payload?.turn?.status || record.payload?.status
          : null;
      const stopTypingEvent =
        record.event === "turn.completed" ||
        ["failed", "canceled", "interrupted"].includes(lifecycleStatus);
      if (typingPaused && record.event !== "approval.required" && !stopTypingEvent) {
        typingPaused = false;
        void tickTyping();
      }

      if (record.event === "item.delta" && record.payload?.kind === "agent_message") {
        responseText += record.payload.delta || "";
        const now = Date.now();
        if (responseText.length > config.maxReplyChars && now - sentProgressAt > 15000) {
          await sendTurnText(chatId, responseText.slice(0, config.maxReplyChars));
          responseText = responseText.slice(config.maxReplyChars);
          sentProgressAt = now;
        }
      }

      if (record.event === "approval.required") {
        typingPaused = true;
        const approval = record.payload || {};
        const approvalId = approval.approval_id || approval.id;
        if (!approvalId) {
          await sendTurnText(
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
        await sendTurnText(
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
        typingPaused = true;
        const turn = record.payload?.turn || {};
        const status = turn.status || "completed";
        const error = turn.error ? `\n${turn.error}` : "";
        if (status !== "completed") {
          await sendTurnText(chatId, `Turn ${status}.${error}`.trim(), {
            replyMarkup: controlKeyboard()
          });
        } else {
          await sendTurnText(chatId, responseText.trim() || "Turn completed.", {
            replyMarkup: controlKeyboard()
          });
        }
        return;
      }

      if (record.event === "turn.lifecycle") {
        if (["failed", "canceled", "interrupted"].includes(lifecycleStatus)) {
          typingPaused = true;
          await sendTurnText(chatId, `Turn ${lifecycleStatus}.`, { replyMarkup: controlKeyboard() });
          return;
        }
      }
    }
  } catch (error) {
    if (error.name === "AbortError") {
      if (timedOut) {
        await sendTurnText(chatId, `Turn timed out after ${Math.round(config.turnTimeoutMs / 1000)}s.`);
      } else if (!stopping) {
        await sendTurnText(chatId, "Turn stream aborted.");
      }
      return;
    }
    throw error;
  } finally {
    clearInterval(typingTimer);
    clearTimeout(timeout);
    options.signal?.removeEventListener("abort", abortFromCaller);
    await flushLastSeq(true);
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
      ...telegramMessageBody(chunk, { markdown: true, maxChars: config.maxReplyChars }),
      disable_web_page_preview: true
    };
    if (options.replyMarkup && index === chunks.length - 1) {
      body.reply_markup = options.replyMarkup;
    }
    try {
      await telegramApi("sendMessage", body);
    } catch (error) {
      if (!isTelegramMarkdownParseError(error)) throw error;
      const fallbackBody = {
        chat_id: chatId,
        ...telegramMessageBody(chunk, { markdown: false, maxChars: config.maxReplyChars }),
        disable_web_page_preview: true
      };
      if (options.replyMarkup && index === chunks.length - 1) {
        fallbackBody.reply_markup = options.replyMarkup;
      }
      await telegramApi("sendMessage", fallbackBody);
    }
  }
}

async function sendTypingAction(chatId) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), TYPING_TIMEOUT_MS);
  try {
    await telegramApi(
      "sendChatAction",
      {
        chat_id: chatId,
        action: "typing"
      },
      { signal: controller.signal }
    );
  } finally {
    clearTimeout(timeout);
  }
}

async function sendTurnText(chatId, text, options = {}) {
  try {
    await sendText(chatId, text, options);
  } catch (error) {
    console.error("failed to send Telegram turn update", error);
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

async function telegramApi(method, body = {}, options = {}) {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await telegramApiOnce(method, body, options);
    } catch (error) {
      const retryMs = method === "sendMessage" ? telegramSendRetryDelayMs(error, attempt) : null;
      if (retryMs == null) throw error;
      console.warn(
        `Telegram ${method} failed: ${error.message}. Retrying in ${Math.round(retryMs / 1000)}s.`
      );
      await delay(retryMs);
    }
  }
}

async function telegramApiOnce(method, body = {}, options = {}) {
  const response = await fetch(`${config.apiBase}/bot${config.botToken}/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: options.signal
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
