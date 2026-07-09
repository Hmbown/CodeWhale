import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";

import {
  getLoginQR,
  waitForLogin,
  getUpdates,
  sendMessage,
  getConfig,
  notifyStart,
  notifyStop,
  ILinkLoginBase,
  parseList,
  parseBool,
  envFirst,
  extractText,
  parseCommand,
  commandAction,
  preservedChatStateFields,
  splitMessage,
  compactRuntimeError,
  latestRunningTurn,
  activeTurnBlock,
  helpText,
} from "./lib.mjs";
import { ThreadStore as CoreThreadStore } from "../../bridge-core/src/lib.mjs";

// ============================================================================
// ThreadStore — JSON 文件持久化（与 feishu/telegram/wechat bridge 一致）
// ============================================================================

class ThreadStore extends CoreThreadStore {
  constructor(filePath) {
    super(filePath, { messageLimit: 500 });
  }
}

// ============================================================================
// 账号持久化
// ============================================================================

function resolveAccountPath(stateDir) {
  return path.join(stateDir, "account.json");
}

async function loadAccount(stateDir) {
  const p = resolveAccountPath(stateDir);
  try {
    const raw = await fs.readFile(p, "utf8");
    return JSON.parse(raw);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return null;
  }
}

async function saveAccount(stateDir, account) {
  const p = resolveAccountPath(stateDir);
  await fs.mkdir(path.dirname(p), { recursive: true, mode: 0o700 });
  const tmp = `${p}.tmp`;
  await fs.writeFile(tmp, `${JSON.stringify(account, null, 2)}\n`, {
    mode: 0o600,
  });
  await fs.rename(tmp, p);
}

// ============================================================================
// 配置
// ============================================================================

function requiredEnv(name) {
  const value = process.env[name];
  if (!value || !value.trim()) {
    console.error(`Missing required env: ${name}`);
    process.exit(1);
  }
  return value.trim();
}

function requiredEnvFirst(...names) {
  const value = envFirst(process.env, ...names);
  if (!value) {
    console.error(`Missing required env: one of ${names.join(", ")}`);
    process.exit(1);
  }
  return value;
}

// WEIXIN_* is the canonical spelling; the historical WEXIN_* typo names are
// still honored as deprecated aliases (each warns once per process).
const warnedEnvAliases = new Set();

function weixinEnv(name) {
  const legacy = `WEXIN_${name.slice("WEIXIN_".length)}`;
  const primary = envFirst(process.env, name);
  if (primary) return primary;
  const fallback = envFirst(process.env, legacy);
  if (fallback && !warnedEnvAliases.has(legacy)) {
    warnedEnvAliases.add(legacy);
    console.warn(`${legacy} is deprecated; rename it to ${name}.`);
  }
  return fallback;
}

const config = {
  runtimeUrl: (
    envFirst(process.env, "CODEWHALE_RUNTIME_URL", "DEEPSEEK_RUNTIME_URL") ||
    "http://127.0.0.1:7878"
  ).replace(/\/+$/, ""),
  runtimeToken: requiredEnvFirst(
    "CODEWHALE_RUNTIME_TOKEN",
    "DEEPSEEK_RUNTIME_TOKEN"
  ),
  workspace:
    envFirst(process.env, "CODEWHALE_WORKSPACE", "DEEPSEEK_WORKSPACE") ||
    process.cwd(),
  model:
    envFirst(process.env, "CODEWHALE_MODEL", "DEEPSEEK_MODEL") || "auto",
  mode:
    envFirst(process.env, "CODEWHALE_MODE", "DEEPSEEK_MODE") || "agent",
  allowShell: parseBool(
    envFirst(
      process.env,
      "CODEWHALE_ALLOW_SHELL",
      "DEEPSEEK_ALLOW_SHELL"
    ),
    true
  ),
  trustMode: parseBool(
    envFirst(
      process.env,
      "CODEWHALE_TRUST_MODE",
      "DEEPSEEK_TRUST_MODE"
    ),
    false
  ),
  autoApprove: parseBool(
    envFirst(
      process.env,
      "CODEWHALE_AUTO_APPROVE",
      "DEEPSEEK_AUTO_APPROVE"
    ),
    false
  ),
  allowlist: parseList(
    weixinEnv("WEIXIN_CHAT_ALLOWLIST") ||
      envFirst(
        process.env,
        "CODEWHALE_CHAT_ALLOWLIST",
        "DEEPSEEK_CHAT_ALLOWLIST"
      )
  ),
  allowUnlisted: parseBool(
    weixinEnv("WEIXIN_ALLOW_UNLISTED") ||
      envFirst(
        process.env,
        "CODEWHALE_ALLOW_UNLISTED",
        "DEEPSEEK_ALLOW_UNLISTED"
      ),
    false
  ),
  stateDir:
    weixinEnv("WEIXIN_STATE_DIR") ||
    "/var/lib/codewhale-weixin-bot-bridge",
  threadMapPath:
    weixinEnv("WEIXIN_THREAD_MAP_PATH") ||
    "/var/lib/codewhale-weixin-bot-bridge/thread-map.json",
  maxReplyChars: Number(weixinEnv("WEIXIN_MAX_REPLY_CHARS") || 3500),
  longPollTimeoutMs: Number(
    weixinEnv("WEIXIN_LONGPOLL_TIMEOUT_MS") || 35000
  ),
  turnTimeoutMs: Number(
    envFirst(
      process.env,
      "CODEWHALE_TURN_TIMEOUT_MS",
      "DEEPSEEK_TURN_TIMEOUT_MS"
    ) || 900000
  ),
};

// ============================================================================
// Runtime API 工具
// ============================================================================

function authHeaders() {
  return {
    Authorization: `Bearer ${config.runtimeToken}`,
    "Content-Type": "application/json",
  };
}

async function readJsonSafe(response) {
  try {
    return await response.json();
  } catch {
    return null;
  }
}

async function runtimeJson(subPath, { method = "GET", body = null, auth = true } = {}) {
  const url = `${config.runtimeUrl}${subPath}`;
  const options = { method, headers: auth ? authHeaders() : {} };
  if (body) options.body = JSON.stringify(body);
  const response = await fetch(url, options);
  const result = await readJsonSafe(response);
  if (!response.ok) {
    throw new Error(compactRuntimeError(response.status, result));
  }
  return result;
}

async function* readSse(response) {
  let buffer = "";
  for await (const chunk of response.body) {
    buffer += new TextDecoder().decode(chunk, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() || "";
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      if (trimmed.startsWith("data:")) {
        yield { data: trimmed.slice(5).trim() };
      } else if (trimmed.startsWith("event:")) {
        yield { event: trimmed.slice(6).trim() };
      } else if (trimmed.startsWith("id:")) {
        yield { id: trimmed.slice(3).trim() };
      }
    }
  }
}

// ============================================================================
// 消息发送 — 通过 iLink sendMessage
// ============================================================================

async function sendText(chatId, text) {
  if (!botAccount) {
    console.error("sendText: bot not logged in");
    return;
  }
  const chunks = splitMessage(text, config.maxReplyChars);
  for (const chunk of chunks) {
    await sendMessage({
      baseUrl: botAccount.baseUrl,
      token: botAccount.token,
      body: {
        msg: {
          to_user_id: chatId,
          client_id: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`,
          message_type: 2, // BOT
          message_state: 2, // FINISH
          item_list: [{ type: 1, text_item: { text: chunk } }],
          context_token: await getContextToken(chatId),
        },
      },
    });
  }
}

async function getContextToken(chatId) {
  const state = await threadStore.getChat(chatId);
  return state?.contextToken || undefined;
}

// ============================================================================
// 命令处理（与 feishu/telegram/wechat bridge 一致）
// ============================================================================

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
    case "set_model":
      await setChatModel(chatId, action.modelName);
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
        "You are being controlled from a WeChat phone chat via iLink Bot. Keep status updates concise. Ask for tool approvals when needed; do not assume mobile messages imply blanket approval.",
    },
  });

  const state = {
    ...preservedChatStateFields(existing),
    threadId: thread.id,
    lastSeq: 0,
    activeTurnId: null,
    updatedAt: new Date().toISOString(),
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
  const effectiveModel = state?.model || config.model;
  const detail = await runtimeJson(
    `/v1/threads/${encodeURIComponent(state.threadId)}`
  );
  const activeBlock = activeTurnBlock(detail, state);
  if (activeBlock) {
    await threadStore.patchChat(chatId, {
      activeTurnId: activeBlock.turnId,
      updatedAt: new Date().toISOString(),
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
        model: effectiveModel,
        mode: config.mode,
        allow_shell: config.allowShell,
        trust_mode: config.trustMode,
        auto_approve: config.autoApprove,
      },
    }
  );

  const turnId = turnResponse.turn?.id;
  await threadStore.patchChat(chatId, {
    activeTurnId: turnId || null,
    lastSeq: sinceSeq,
    updatedAt: new Date().toISOString(),
  });
  await sendText(chatId, `Started turn ${turnId || "(unknown)"}`);

  try {
    await streamTurnEvents(chatId, state.threadId, turnId, sinceSeq);
  } finally {
    await threadStore.patchChat(chatId, {
      activeTurnId: null,
      updatedAt: new Date().toISOString(),
    });
  }
}

async function streamTurnEvents(chatId, threadId, turnId, sinceSeq) {
  const controller = new AbortController();
  const timeout = setTimeout(
    () => controller.abort(),
    config.turnTimeoutMs
  );
  let responseText = "";
  let latestSeq = sinceSeq;
  let sentProgressAt = Date.now();

  try {
    const response = await fetch(
      `${config.runtimeUrl}/v1/threads/${encodeURIComponent(threadId)}/events?since_seq=${sinceSeq}`,
      {
        headers: authHeaders(),
        signal: controller.signal,
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

      if (
        record.event === "item.delta" &&
        record.payload?.kind === "agent_message"
      ) {
        responseText += record.payload.delta || "";
        const now = Date.now();
        if (
          responseText.length > config.maxReplyChars &&
          now - sentProgressAt > 15000
        ) {
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
              "No approval_id was provided by the runtime; use /status and retry from the TUI.",
            ]
              .filter(Boolean)
              .join("\n")
          );
        } else {
          await sendText(
            chatId,
            [
              "Approval required",
              `tool=${approval.tool_name || "unknown"}`,
              `approval_id=${approvalId}`,
              approval.description || "",
              "",
              `Reply /allow ${approvalId} or /deny ${approvalId}`,
            ]
              .filter(Boolean)
              .join("\n")
          );
        }
      }

      if (record.event === "turn.completed") {
        const turn = record.payload?.turn || {};
        const status = turn.status || "completed";
        const error = turn.error ? `\n${turn.error}` : "";
        if (status !== "completed") {
          await sendText(chatId, `Turn ${status}.${error}`.trim());
        } else {
          await sendText(
            chatId,
            responseText.trim() || "Turn completed."
          );
        }
        return;
      }

      if (record.event === "turn.lifecycle") {
        const status =
          record.payload?.turn?.status || record.payload?.status;
        if (["failed", "canceled", "interrupted"].includes(status)) {
          await sendText(chatId, `Turn ${status}.`);
          return;
        }
      }
    }
  } catch (error) {
    if (error.name === "AbortError") {
      await sendText(
        chatId,
        `Turn timed out after ${Math.round(config.turnTimeoutMs / 1000)}s.`
      );
      return;
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function sendStatus(chatId) {
  try {
    const [health, runtimeInfo, workspace] = await Promise.all([
      runtimeJson("/health", { auth: false }),
      runtimeJson("/v1/runtime/info"),
      runtimeJson("/v1/workspace/status"),
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
        `staged=${workspace.staged} unstaged=${workspace.unstaged} untracked=${workspace.untracked}`,
      ]
        .filter(Boolean)
        .join("\n")
    );
  } catch (error) {
    await sendText(chatId, `Status check failed: ${error.message}`);
  }
}

async function sendThreads(chatId) {
  try {
    const threads = await runtimeJson(
      "/v1/threads/summary?limit=8&include_archived=true"
    );
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
  } catch (error) {
    await sendText(chatId, `Thread listing failed: ${error.message}`);
  }
}

async function resumeThread(chatId, args) {
  const threadId = args.trim();
  if (!threadId) {
    await sendText(chatId, "Usage: /resume <thread_id>");
    return;
  }
  try {
    const detail = await runtimeJson(
      `/v1/threads/${encodeURIComponent(threadId)}`
    );
    const existing = await threadStore.getChat(chatId);
    await threadStore.setChat(chatId, {
      ...preservedChatStateFields(existing),
      threadId,
      lastSeq: Number(detail.latest_seq || 0),
      activeTurnId: null,
      updatedAt: new Date().toISOString(),
    });
    await sendText(chatId, `Resumed thread ${threadId}`);
  } catch (error) {
    await sendText(chatId, `Resume failed: ${error.message}`);
  }
}

async function interruptActiveTurn(chatId) {
  const state = await threadStore.getChat(chatId);
  if (!state?.threadId) {
    await sendText(chatId, "No runtime thread recorded for this chat.");
    return;
  }
  try {
    const detail = await runtimeJson(
      `/v1/threads/${encodeURIComponent(state.threadId)}`
    );
    const runningTurn = latestRunningTurn(detail);
    const turnId = state.activeTurnId || runningTurn?.id;
    if (!turnId) {
      await sendText(chatId, "No active turn recorded for this chat.");
      return;
    }
    await runtimeJson(
      `/v1/threads/${encodeURIComponent(state.threadId)}/turns/${encodeURIComponent(turnId)}/interrupt`,
      { method: "POST" }
    );
    await threadStore.patchChat(chatId, {
      activeTurnId: turnId,
      updatedAt: new Date().toISOString(),
    });
    await sendText(chatId, `Interrupt requested for ${turnId}`);
  } catch (error) {
    await sendText(chatId, `Interrupt failed: ${error.message}`);
  }
}

async function compactThread(chatId) {
  try {
    const state = await ensureThread(chatId);
    const result = await runtimeJson(
      `/v1/threads/${encodeURIComponent(state.threadId)}/compact`,
      {
        method: "POST",
        body: { reason: "weixin-bot bridge request" },
      }
    );
    await sendText(
      chatId,
      `Compaction started: ${result.turn?.id || "unknown turn"}`
    );
  } catch (error) {
    await sendText(chatId, `Compact failed: ${error.message}`);
  }
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
  try {
    await runtimeJson(
      `/v1/approvals/${encodeURIComponent(approvalId)}`,
      {
        method: "POST",
        body: { decision, remember },
      }
    );
    await sendText(
      chatId,
      `Approval ${approvalId}: ${decision}${remember ? " and remember" : ""}`
    );
  } catch (error) {
    await sendText(chatId, `Approval failed: ${error.message}`);
  }
}

async function setChatModel(chatId, modelName) {
  if (!modelName || modelName === "default") {
    await threadStore.patchChat(chatId, {
      model: null,
      updatedAt: new Date().toISOString(),
    });
    await sendText(
      chatId,
      `Reset per-chat model. Using bridge default: ${config.model}`
    );
    return;
  }
  await threadStore.patchChat(chatId, {
    model: modelName,
    updatedAt: new Date().toISOString(),
  });
  await sendText(chatId, `Per-chat model set to: ${modelName}`);
}

// ============================================================================
// 主循环 — 长轮询 getUpdates
// ============================================================================

let botAccount = null;
let stopping = false;
let threadStore;
let stopSignal = null;

function resolveSyncBufPath(stateDir) {
  return path.join(stateDir, "sync-buf.txt");
}

async function loadSyncBuf(stateDir) {
  const p = resolveSyncBufPath(stateDir);
  try {
    return await fs.readFile(p, "utf8");
  } catch {
    return "";
  }
}

async function saveSyncBuf(stateDir, buf) {
  const p = resolveSyncBufPath(stateDir);
  const tmp = `${p}.tmp`;
  await fs.writeFile(tmp, buf, { mode: 0o600 });
  await fs.rename(tmp, p);
}

async function monitorLoop() {
  const { baseUrl, token } = botAccount;
  let getUpdatesBuf = await loadSyncBuf(config.stateDir);
  let nextTimeoutMs = config.longPollTimeoutMs;
  let consecutiveFailures = 0;

  console.log(`Monitor started: baseUrl=${baseUrl} timeoutMs=${nextTimeoutMs}`);

  while (!stopping) {
    try {
      const abortController = new AbortController();
      const timer = setTimeout(
        () => abortController.abort(),
        nextTimeoutMs + 5000
      );

      const resp = await getUpdates({
        baseUrl,
        token,
        get_updates_buf: getUpdatesBuf,
        timeoutMs: nextTimeoutMs,
        signal: abortController.signal,
      });

      clearTimeout(timer);

      if (resp.longpolling_timeout_ms) {
        nextTimeoutMs = resp.longpolling_timeout_ms;
      }

      // 检查错误
      const isApiError =
        (resp.ret !== undefined && resp.ret !== 0) ||
        (resp.errcode !== undefined && resp.errcode !== 0);

      if (isApiError) {
        consecutiveFailures += 1;
        console.error(
          `getUpdates error: ret=${resp.ret} errcode=${resp.errcode} errmsg=${resp.errmsg}`
        );
        if (consecutiveFailures >= 3) {
          console.error("3 consecutive failures, backing off 30s");
          await sleep(30000);
          consecutiveFailures = 0;
        } else {
          await sleep(2000);
        }
        continue;
      }

      consecutiveFailures = 0;

      // 保存游标
      if (resp.get_updates_buf) {
        getUpdatesBuf = resp.get_updates_buf;
        await saveSyncBuf(config.stateDir, getUpdatesBuf);
      }

      // 处理消息
      const msgs = resp.msgs || [];
      for (const msg of msgs) {
        const fromUser = msg.from_user_id || "";
        const messageId = String(msg.message_id || "");

        if (!fromUser) continue;

        const msgKey = `${fromUser}:${messageId}`;
        if (await threadStore.recordMessage(msgKey)) continue;

        // 保存 context_token
        if (msg.context_token) {
          await threadStore.patchChat(fromUser, {
            contextToken: msg.context_token,
            updatedAt: new Date().toISOString(),
          });
        }

        // 提取文本
        const text = extractText(msg.item_list);

        if (!text) {
          await sendText(
            fromUser,
            "仅支持文本消息。图片/语音/视频/文件暂不支持。"
          );
          continue;
        }

        console.log(
          `[inbound] from=${fromUser} text=${text.slice(0, 100)}`
        );

        // 白名单检查
        if (!isAllowed(fromUser)) {
          await sendText(
            fromUser,
            [
              "This WeChat user is not in WEIXIN_CHAT_ALLOWLIST.",
              `user_id=${fromUser}`,
              "",
              "For first pairing, add this user_id to WEIXIN_CHAT_ALLOWLIST, or temporarily set WEIXIN_ALLOW_UNLISTED=true.",
            ].join("\n")
          );
          continue;
        }

        // 命令路由
        const command = parseCommand(text);
        await handleCommand(fromUser, command).catch((error) => {
          console.error(
            `failed to handle command from=${fromUser} text=${text.slice(0, 100)}`,
            error
          );
        });
      }
    } catch (error) {
      if (error.name === "AbortError" || error.message?.includes("abort")) {
        // 长轮询超时是正常的，立即重试
        continue;
      }
      if (stopping) break;

      consecutiveFailures += 1;
      console.error(
        `getUpdates exception (${consecutiveFailures}/3):`,
        error.message
      );
      if (consecutiveFailures >= 3) {
        console.error("3 consecutive exceptions, backing off 30s");
        await sleep(30000);
        consecutiveFailures = 0;
      } else {
        await sleep(2000);
      }
    }
  }
}

function isAllowed(fromUser) {
  if (config.allowUnlisted) return true;
  const allowed = new Set(config.allowlist);
  return allowed.has(fromUser);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ============================================================================
// 启动流程 — QR 登录 → 长轮询
// ============================================================================

async function main() {
  console.log("Starting CodeWhale Weixin Bot Bridge");
  console.log(`Runtime: ${config.runtimeUrl}`);
  console.log(`Workspace: ${config.workspace}`);
  console.log(`State dir: ${config.stateDir}`);

  // 初始化 ThreadStore
  threadStore = await ThreadStore.open(config.threadMapPath);

  // 尝试加载已有账号
  botAccount = await loadAccount(config.stateDir);

  if (botAccount?.token) {
    console.log("Loaded existing bot account, trying to resume...");
    console.log(`  accountId: ${botAccount.accountId}`);
    console.log(`  baseUrl: ${botAccount.baseUrl}`);
  } else {
    // QR 登录
    console.log("No bot account found. Starting QR login...");
    console.log("");

    const { qrcodeUrl, sessionKey } = await getLoginQR();
    console.log("请用微信扫描以下二维码登录：");
    console.log(qrcodeUrl);
    console.log("");

    const result = await waitForLogin({ sessionKey, timeoutMs: 300_000 });

    if (!result.connected) {
      console.error(`Login failed: ${result.message}`);
      process.exit(1);
    }

    botAccount = {
      accountId: result.accountId,
      token: result.botToken,
      baseUrl: result.baseUrl,
      userId: result.userId,
    };

    await saveAccount(config.stateDir, botAccount);
    console.log(`✅ Login successful! accountId=${botAccount.accountId}`);
  }

  // 通知上线
  try {
    const startResp = await notifyStart({
      baseUrl: botAccount.baseUrl,
      token: botAccount.token,
    });
    if (startResp.ret && startResp.ret !== 0) {
      console.warn(`notifyStart: ret=${startResp.ret} errmsg=${startResp.errmsg}`);
    } else {
      console.log("notifyStart: OK");
    }
  } catch (error) {
    console.error("notifyStart failed:", error.message);
  }

  // 信号处理
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);

  if (!config.allowlist.length && !config.allowUnlisted) {
    console.log(
      "No allowlist configured. Incoming chats will receive their user IDs and be refused."
    );
  }

  // 进入长轮询循环
  await monitorLoop();

  console.log("Bridge stopped.");
}

async function shutdown() {
  if (stopping) return;
  stopping = true;
  console.log("Shutting down...");

  if (botAccount?.token) {
    try {
      const stopResp = await notifyStop({
        baseUrl: botAccount.baseUrl,
        token: botAccount.token,
      });
      console.log(
        `notifyStop: ret=${stopResp.ret} errmsg=${stopResp.errmsg ?? "OK"}`
      );
    } catch (error) {
      console.error("notifyStop failed:", error.message);
    }
  }

  setTimeout(() => process.exit(0), 2000);
}

main().catch((error) => {
  console.error("Fatal error:", error);
  process.exit(1);
});
