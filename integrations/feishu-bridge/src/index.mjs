import fs from "node:fs/promises";
import path from "node:path";
import * as Lark from "@larksuiteoapi/node-sdk";

import {
  activeTurnBlock,
  commandAction,
  compactRuntimeError,
  helpText,
  incomingIdentity,
  isAllowed,
  latestRunningTurn,
  pairingRefusalText,
  parseBool,
  parseCommand,
  parseList,
  parseApprovalDecisionArgs,
  parseTextContent,
  splitMessage,
  stripGroupPrefix
} from "./lib.mjs";

class ThreadStore {
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
      const raw = await fs.readFile(this.filePath, "utf8");
      this.data = JSON.parse(raw);
      if (!this.data.chats) this.data.chats = {};
      if (!this.data.messages) this.data.messages = [];
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }

  async recordMessage(messageId) {
    if (!messageId) return false;
    if (!Array.isArray(this.data.messages)) this.data.messages = [];
    if (this.data.messages.includes(messageId)) return true;
    this.data.messages.push(messageId);
    this.data.messages = this.data.messages.slice(-200);
    await this.save();
    return false;
  }

  async getChat(chatId) {
    return this.data.chats[chatId] || null;
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
    await fs.mkdir(dir, { recursive: true, mode: 0o700 });
    const tmp = `${this.filePath}.tmp`;
    await fs.writeFile(tmp, `${JSON.stringify(this.data, null, 2)}\n`, { mode: 0o600 });
    await fs.rename(tmp, this.filePath);
  }
}

const config = {
  appId: requiredEnv("FEISHU_APP_ID"),
  appSecret: requiredEnv("FEISHU_APP_SECRET"),
  domain: process.env.FEISHU_DOMAIN || "feishu",
  runtimeUrl: (process.env.DEEPSEEK_RUNTIME_URL || "http://127.0.0.1:7878").replace(/\/+$/, ""),
  runtimeToken: requiredEnv("DEEPSEEK_RUNTIME_TOKEN"),
  workspace: process.env.DEEPSEEK_WORKSPACE || process.cwd(),
  model: process.env.DEEPSEEK_MODEL || "auto",
  mode: process.env.DEEPSEEK_MODE || "agent",
  allowShell: parseBool(process.env.DEEPSEEK_ALLOW_SHELL, true),
  trustMode: parseBool(process.env.DEEPSEEK_TRUST_MODE, false),
  autoApprove: parseBool(process.env.DEEPSEEK_AUTO_APPROVE, false),
  allowlist: parseList(process.env.DEEPSEEK_CHAT_ALLOWLIST),
  allowUnlisted: parseBool(process.env.DEEPSEEK_ALLOW_UNLISTED, false),
  threadMapPath:
    process.env.FEISHU_THREAD_MAP_PATH ||
    "/var/lib/deepseek-feishu-bridge/thread-map.json",
  allowGroups: parseBool(process.env.FEISHU_ALLOW_GROUPS, false),
  requirePrefixInGroup: parseBool(process.env.FEISHU_REQUIRE_PREFIX_IN_GROUP, true),
  groupPrefix: process.env.FEISHU_GROUP_PREFIX || "/ds",
  maxReplyChars: Number(process.env.FEISHU_MAX_REPLY_CHARS || 3500),
  turnTimeoutMs: Number(process.env.DEEPSEEK_TURN_TIMEOUT_MS || 900000)
};

const sdkConfig = {
  appId: config.appId,
  appSecret: config.appSecret,
  domain: resolveLarkDomain(config.domain)
};

const client = new Lark.Client(sdkConfig);
const wsClient = new Lark.WSClient({
  ...sdkConfig,
  loggerLevel: Lark.LoggerLevel?.info
});

const threadStore = await ThreadStore.open(config.threadMapPath);

const dispatcher = new Lark.EventDispatcher({}).register({
  "im.message.receive_v1": async (data) => {
    void handleIncomingMessage(data).catch((error) => {
      console.error("failed to handle incoming Feishu message", error);
    });
  }
});

console.log("Starting DeepSeek Feishu bridge");
console.log(`Runtime: ${config.runtimeUrl}`);
console.log(`Workspace: ${config.workspace}`);
if (!config.allowlist.length && !config.allowUnlisted) {
  console.log("No allowlist configured. Incoming chats will receive their IDs and be refused.");
}

wsClient.start({ eventDispatcher: dispatcher });

async function handleIncomingMessage(event) {
  const identity = incomingIdentity(event);
  if (!identity.chatId) return;

  if (identity.messageType && identity.messageType !== "text") {
    await sendText(identity.chatId, "Only text messages are supported in this first bridge.");
    return;
  }

  const rawText = parseTextContent(event.message?.content || "");
  const scoped = stripGroupPrefix(rawText, {
    chatType: identity.chatType,
    requirePrefix: config.requirePrefixInGroup,
    prefix: config.groupPrefix
  });
  if (!scoped.accepted) return;

  if (identity.messageId && (await threadStore.recordMessage(identity.messageId))) {
    return;
  }

  if (identity.chatType !== "p2p" && !config.allowGroups) {
    await sendText(
      identity.chatId,
      "Group chat control is disabled for this bridge. DM the bot, or set FEISHU_ALLOW_GROUPS=true and allowlist this chat."
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
      await sendText(chatId, helpText());
      return;
    case "status":
      await sendStatus(chatId);
      return;
    case "threads":
      await sendThreads(chatId);
      return;
    case "new_thread": {
      const state = await ensureThread(chatId, { forceNew: true });
      await sendText(chatId, `Created thread ${state.threadId}`);
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
    case "prompt":
      await runPrompt(chatId, action.prompt);
      return;
    default:
      await sendText(chatId, helpText());
  }
}

async function ensureThread(chatId, { forceNew = false } = {}) {
  const existing = await threadStore.getChat(chatId);
  if (existing?.threadId && !forceNew) return existing;

  const thread = await runtimeJson("/v1/threads", {
    method: "POST",
    body: {
      model: config.model,
      workspace: config.workspace,
      mode: config.mode,
      allow_shell: config.allowShell,
      trust_mode: config.trustMode,
      auto_approve: config.autoApprove,
      archived: false,
      system_prompt:
        "You are being controlled from a Feishu/Lark phone chat. Keep status updates concise. Ask for tool approvals when needed; do not assume mobile messages imply blanket approval."
    }
  });

  const state = {
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
    await sendText(chatId, helpText());
    return;
  }
  const state = await ensureThread(chatId);
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}`);
  const activeBlock = activeTurnBlock(detail, state);
  if (activeBlock) {
    await threadStore.patchChat(chatId, {
      activeTurnId: activeBlock.turnId,
      updatedAt: new Date().toISOString()
    });
    await sendText(chatId, activeBlock.message);
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
        model: config.model,
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
  await sendText(chatId, `Started turn ${turnId || "(unknown)"}`);

  try {
    await streamTurnEvents(chatId, state.threadId, turnId, sinceSeq);
  } finally {
    await threadStore.patchChat(chatId, {
      activeTurnId: null,
      updatedAt: new Date().toISOString()
    });
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
        await sendText(
          chatId,
          [
            "Approval required",
            `tool=${approval.tool_name || "unknown"}`,
            `approval_id=${approval.approval_id || approval.id}`,
            approval.description || "",
            "",
            `Reply /allow ${approval.approval_id || approval.id}`,
            `Reply /deny ${approval.approval_id || approval.id}`
          ]
            .filter(Boolean)
            .join("\n")
        );
      }

      if (record.event === "turn.completed") {
        const turn = record.payload?.turn || {};
        const status = turn.status || "completed";
        const error = turn.error ? `\n${turn.error}` : "";
        if (status !== "completed") {
          await sendText(chatId, `Turn ${status}.${error}`.trim());
        } else {
          await sendText(chatId, responseText.trim() || "Turn completed.");
        }
        return;
      }

      if (record.event === "turn.lifecycle") {
        const status = record.payload?.turn?.status || record.payload?.status;
        if (["failed", "canceled", "interrupted"].includes(status)) {
          await sendText(chatId, `Turn ${status}.`);
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
      .join("\n")
  );
}

async function sendThreads(chatId) {
  const threads = await runtimeJson("/v1/threads/summary?limit=8&include_archived=true");
  if (!threads.length) {
    await sendText(chatId, "No runtime threads yet.");
    return;
  }
  await sendText(
    chatId,
    threads
      .map((thread) => {
        const status = thread.latest_turn_status || "none";
        return `${thread.id} [${status}] ${thread.title || thread.preview || ""}`;
      })
      .join("\n")
  );
}

async function resumeThread(chatId, args) {
  const threadId = args.trim();
  if (!threadId) {
    await sendText(chatId, "Usage: /resume <thread_id>");
    return;
  }
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(threadId)}`);
  await threadStore.setChat(chatId, {
    threadId,
    lastSeq: Number(detail.latest_seq || 0),
    activeTurnId: null,
    updatedAt: new Date().toISOString()
  });
  await sendText(chatId, `Resumed thread ${threadId}`);
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
  await sendText(chatId, `Interrupt requested for ${turnId}`);
}

async function compactThread(chatId) {
  const state = await ensureThread(chatId);
  const result = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}/compact`, {
    method: "POST",
    body: { reason: "phone bridge request" }
  });
  await sendText(chatId, `Compaction started: ${result.turn?.id || "unknown turn"}`);
}

async function decideApproval(chatId, action) {
  const decision = action.decision;
  const { approvalId, remember } =
    action.approvalId != null ? action : parseApprovalDecisionArgs(action.args);
  if (!approvalId) {
    await sendText(chatId, `Usage: /${decision} <approval_id>${decision === "allow" ? " [remember]" : ""}`);
    return;
  }
  await runtimeJson(`/v1/approvals/${encodeURIComponent(approvalId)}`, {
    method: "POST",
    body: { decision, remember }
  });
  await sendText(chatId, `Approval ${approvalId}: ${decision}${remember ? " and remember" : ""}`);
}

async function sendText(chatId, text) {
  const createMessage =
    client.im?.v1?.message?.create?.bind(client.im.v1.message) ||
    client.im?.message?.create?.bind(client.im.message);
  if (!createMessage) {
    throw new Error("Lark SDK client does not expose im message create API");
  }
  for (const chunk of splitMessage(text, config.maxReplyChars)) {
    await createMessage({
      params: { receive_id_type: "chat_id" },
      data: {
        receive_id: chatId,
        msg_type: "text",
        content: JSON.stringify({ text: chunk })
      }
    });
  }
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

function resolveLarkDomain(domain) {
  const normalized = String(domain || "feishu").toLowerCase();
  if (normalized === "lark") return Lark.Domain?.Lark || "https://open.larksuite.com";
  if (normalized === "feishu") return Lark.Domain?.Feishu || "https://open.feishu.cn";
  return domain;
}
