import { WSClient, generateReqId } from "@wecom/aibot-node-sdk";

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
  preservedChatStateFields,
  requiredEnv,
  splitMessage,
  stripGroupPrefix,
  ThreadStore
} from "./lib.mjs";

const config = {
  botId: requiredEnv("WECOM_BOT_ID"),
  botSecret: requiredEnv("WECOM_BOT_SECRET"),
  runtimeUrl: (process.env.CODEWHALE_RUNTIME_URL || "http://127.0.0.1:7878").replace(/\/+$/, ""),
  runtimeToken: requiredEnv("CODEWHALE_RUNTIME_TOKEN"),
  workspace: process.env.CODEWHALE_WORKSPACE || process.cwd(),
  model: process.env.CODEWHALE_MODEL || "auto",
  mode: process.env.CODEWHALE_MODE || "agent",
  allowShell: parseBool(process.env.CODEWHALE_ALLOW_SHELL, true),
  trustMode: parseBool(process.env.CODEWHALE_TRUST_MODE, false),
  autoApprove: parseBool(process.env.CODEWHALE_AUTO_APPROVE, false),
  allowlist: parseList(process.env.WECOM_CHAT_ALLOWLIST),
  allowUnlisted: parseBool(process.env.WECOM_ALLOW_UNLISTED, false),
  threadMapPath: process.env.WECOM_THREAD_MAP_PATH || "/var/lib/codewhale-wecom-bridge/thread-map.json",
  maxReplyChars: Number(process.env.WECOM_MAX_REPLY_CHARS || 3500),
  turnTimeoutMs: Number(process.env.CODEWHALE_TURN_TIMEOUT_MS || 900000)
};

const threadStore = await ThreadStore.open(config.threadMapPath);

const client = new WSClient({
  botId: config.botId,
  secret: config.botSecret
});

client.on("message", async (frame) => {
  try {
    await handleIncomingMessage(frame);
  } catch (error) {
    console.error("Failed to handle WeCom message", error);
  }
});

client.on("event", async (frame) => {
  try {
    await handleEvent(frame);
  } catch (error) {
    console.error("Failed to handle WeCom event", error);
  }
});

client.on("error", (error) => {
  console.error("WeCom client error:", error);
});

console.log("Starting CodeWhale WeCom bridge");
console.log(`Runtime: ${config.runtimeUrl}`);
console.log(`Workspace: ${config.workspace}`);
if (!config.allowlist.length && !config.allowUnlisted) {
  console.log("No allowlist configured. Incoming chats will receive their IDs and be refused.");
}

client.connect();

function replyText(frame, text) {
  const streamId = generateReqId("stream");
  return client.replyStream(frame, streamId, text, true);
}

async function handleIncomingMessage(frame) {
  const body = frame.body || {};
  const identity = incomingIdentity(body);
  console.log(`Incoming message: chatId=${identity.chatId} userId=${identity.userId} chatType=${identity.chatType}`);
  if (!identity.chatId || !identity.messageId) return;

  if (body.msgtype && body.msgtype !== "text") {
    await replyText(frame, "目前仅支持文本消息。");
    return;
  }

  const textContent = body.text?.content || "";
  const scoped = stripGroupPrefix(textContent, {
    chatType: identity.chatType,
    requirePrefix: identity.chatType === "group",
    prefix: "/ds"
  });
  if (!scoped.accepted) return;

  if (!isAllowed(identity, config.allowlist, config.allowUnlisted)) {
    await replyText(frame, pairingRefusalText(identity));
    return;
  }

  const command = parseCommand(scoped.text);
  await handleCommand(identity.chatId, command, frame);
}

async function handleEvent(frame) {
  const body = frame.body || {};
  const eventType = body.event?.eventtype || "";
  if (eventType === "enter_chat") {
    const chatId = body.chatid;
    if (chatId) {
      await client.replyWelcome(frame, { msgtype: "text", text: { content: "欢迎使用 CodeWhale！发送 /help 查看可用命令。" } });
    }
  }
}

async function handleCommand(chatId, command, frame) {
  const action = commandAction(command);
  switch (action.kind) {
    case "help":
      await replyText(frame, helpText());
      return;
    case "status":
      await sendStatus(chatId, frame);
      return;
    case "threads":
      await sendThreads(chatId, frame);
      return;
    case "new_thread": {
      const state = await ensureThread(chatId);
      await replyText(frame, `Created thread ${state.threadId}`);
      return;
    }
    case "resume":
      await resumeThread(chatId, action.threadId, frame);
      return;
    case "interrupt":
      await interruptActiveTurn(chatId, frame);
      return;
    case "compact":
      await compactThread(chatId, frame);
      return;
    case "approval":
      await decideApproval(chatId, action, frame);
      return;
    case "set_model":
      await setChatModel(chatId, action.modelName, frame);
      return;
    case "prompt":
      await runPrompt(chatId, action.prompt, frame);
      return;
    default:
      await replyText(frame, helpText());
  }
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
        "You are being controlled from a WeCom (企业微信) phone chat. Keep status updates concise. Ask for tool approvals when needed; do not assume mobile messages imply blanket approval."
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

async function runPrompt(chatId, prompt, frame) {
  if (!prompt.trim()) {
    await replyText(frame, helpText());
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
    await replyText(frame, activeBlock.message);
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
  await replyText(frame, `Started turn ${turnId || "(unknown)"}`);

  try {
    await streamTurnEvents(chatId, frame, state.threadId, turnId, sinceSeq);
  } finally {
    await threadStore.patchChat(chatId, {
      activeTurnId: null,
      updatedAt: new Date().toISOString()
    });
  }
}

async function streamTurnEvents(chatId, frame, threadId, turnId, sinceSeq) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), config.turnTimeoutMs);
  const streamId = generateReqId("stream");
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
        await client.replyStream(frame, streamId, responseText, false);
      }

      if (record.event === "approval.required") {
        const approval = record.payload || {};
        await replyText(
          frame,
          [
            "审批请求",
            `tool=${approval.tool_name || "unknown"}`,
            `approval_id=${approval.approval_id || approval.id}`,
            approval.description || "",
            "",
            `回复 /allow ${approval.approval_id || approval.id}`,
            `回复 /deny ${approval.approval_id || approval.id}`
          ]
            .filter(Boolean)
            .join("\n")
        );
      }

      if (record.event === "turn.completed") {
        const turn = record.payload?.turn || {};
        const status = turn.status || "completed";
        const error = turn.error ? `\n${turn.error}` : "";
        await client.replyStream(frame, streamId, responseText.trim() || "Turn completed.", true);
        return;
      }

      if (record.event === "turn.lifecycle") {
        const status = record.payload?.turn?.status || record.payload?.status;
        if (["failed", "canceled", "interrupted"].includes(status)) {
          await client.replyStream(frame, streamId, `Turn ${status}.`, true);
          return;
        }
      }
    }
  } catch (error) {
    if (error.name === "AbortError") {
      await replyText(frame, `Turn timed out after ${Math.round(config.turnTimeoutMs / 1000)}s.`);
      return;
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function sendStatus(chatId, frame) {
  const [health, runtimeInfo, workspace] = await Promise.all([
    runtimeJson("/health", { auth: false }),
    runtimeJson("/v1/runtime/info"),
    runtimeJson("/v1/workspace/status")
  ]);
  await replyText(
    frame,
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

async function sendThreads(chatId, frame) {
  const threads = await runtimeJson("/v1/threads/summary?limit=8&include_archived=true");
  if (!threads.length) {
    await replyText(frame, "No runtime threads yet.");
    return;
  }
  await replyText(
    frame,
    threads
      .map((thread) => {
        const status = thread.latest_turn_status || "none";
        return `${thread.id} [${status}] ${thread.title || thread.preview || ""}`;
      })
      .join("\n")
  );
}

async function resumeThread(chatId, args, frame) {
  const threadId = args.trim();
  if (!threadId) {
    await replyText(frame, "Usage: /resume <thread_id>");
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
  await replyText(frame, `Resumed thread ${threadId}`);
}

async function interruptActiveTurn(chatId, frame) {
  const state = await threadStore.getChat(chatId);
  if (!state?.threadId) {
    await replyText(frame, "No runtime thread recorded for this chat.");
    return;
  }
  const detail = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}`);
  const runningTurn = latestRunningTurn(detail);
  const turnId = state.activeTurnId || runningTurn?.id;
  if (!turnId) {
    await replyText(frame, "No active turn recorded for this chat.");
    return;
  }
  await runtimeJson(
    `/v1/threads/${encodeURIComponent(state.threadId)}/turns/${encodeURIComponent(turnId)}/interrupt`,
    { method: "POST" }
  );
  await threadStore.patchChat(chatId, {
    activeTurnId: turnId,
    updatedAt: new Date().toISOString()
  });
  await replyText(frame, `Interrupt requested for ${turnId}`);
}

async function compactThread(chatId, frame) {
  const state = await ensureThread(chatId);
  const result = await runtimeJson(`/v1/threads/${encodeURIComponent(state.threadId)}/compact`, {
    method: "POST",
    body: { reason: "phone bridge request" }
  });
  await replyText(frame, `Compaction started: ${result.turn?.id || "unknown turn"}`);
}

async function decideApproval(chatId, action, frame) {
  const decision = action.decision;
  const { approvalId, remember } =
    action.approvalId != null ? action : parseApprovalDecisionArgs(action.args);
  if (!approvalId) {
    await replyText(frame, `Usage: /${decision} <approval_id>${decision === "allow" ? " [remember]" : ""}`);
    return;
  }
  await runtimeJson(`/v1/approvals/${encodeURIComponent(approvalId)}`, {
    method: "POST",
    body: { decision, remember }
  });
  await replyText(frame, `Approval ${approvalId}: ${decision}${remember ? " and remember" : ""}`);
}

async function setChatModel(chatId, modelName, frame) {
  if (!modelName || modelName === "default") {
    await threadStore.patchChat(chatId, {
      model: null,
      updatedAt: new Date().toISOString()
    });
    await replyText(frame, `Reset per-chat model. Using bridge default: ${config.model}`);
    return;
  }
  await threadStore.patchChat(chatId, {
    model: modelName,
    updatedAt: new Date().toISOString()
  });
  await replyText(frame, `Per-chat model set to: ${modelName}`);
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
