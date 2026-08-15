export const STREAM_EVENT_NAMES = [
  "thread.started",
  "thread.updated",
  "thread.forked",
  "turn.started",
  "turn.lifecycle",
  "turn.steered",
  "turn.interrupt_requested",
  "turn.completed",
  "item.started",
  "item.delta",
  "item.completed",
  "item.failed",
  "item.interrupted",
  "approval.required",
  "approval.decided",
  "approval.timeout",
  "user_input.required",
  "user_input.answered",
  "user_input.canceled",
  "sandbox.denied",
  "agent.spawned",
  "agent.progress",
  "agent.completed",
  "agent.list",
  "tool_call.requested",
  "tool_call.resolved",
  "tool_call.canceled",
  "tool_call.timeout",
];

export function createThreadState(threadId = "") {
  return {
    threadId,
    thread: null,
    turns: new Map(),
    turnOrder: [],
    items: new Map(),
    itemOrder: [],
    latestSeq: 0,
    approvals: new Map(),
    userInputs: new Map(),
    dynamicToolCalls: new Map(),
  };
}

export function applySnapshot(state, detail, expectedThreadId = state.threadId) {
  if (!detail || !detail.thread || detail.thread.id !== expectedThreadId) {
    return false;
  }
  state.threadId = expectedThreadId;
  state.thread = detail.thread;
  state.turns = new Map();
  state.turnOrder = [];
  for (const turn of Array.isArray(detail.turns) ? detail.turns : []) {
    if (!turn || !turn.id) continue;
    state.turns.set(turn.id, turn);
    state.turnOrder.push(turn.id);
  }
  state.items = new Map();
  state.itemOrder = [];
  for (const item of Array.isArray(detail.items) ? detail.items : []) {
    if (!item || !item.id) continue;
    state.items.set(item.id, item);
    state.itemOrder.push(item.id);
  }
  state.latestSeq = normalizedSequence(detail.latest_seq);
  state.approvals = new Map();
  for (const approval of Array.isArray(detail.pending_approvals) ? detail.pending_approvals : []) {
    const approvalId = approval?.approval_id || approval?.id;
    if (approvalId) state.approvals.set(approvalId, approval);
  }
  state.userInputs = new Map();
  for (const input of Array.isArray(detail.pending_user_inputs) ? detail.pending_user_inputs : []) {
    const inputId = input?.input_id || input?.id;
    if (inputId) state.userInputs.set(inputId, input);
  }
  state.dynamicToolCalls = new Map();
  for (const call of Array.isArray(detail.pending_dynamic_tool_calls) ? detail.pending_dynamic_tool_calls : []) {
    if (call?.call_id) state.dynamicToolCalls.set(call.call_id, call);
  }
  return true;
}

export function applyRuntimeEvent(state, envelope) {
  if (runtimeEventContinuity(state, envelope) !== "next") {
    return false;
  }
  const sequence = normalizedSequence(envelope.seq);
  state.latestSeq = sequence;

  const eventName = envelope.event || envelope.kind || "";
  const payload = envelope.payload && typeof envelope.payload === "object"
    ? envelope.payload
    : {};

  if (
    (eventName === "thread.started" || eventName === "thread.updated" || eventName === "thread.forked")
    && payload.thread
  ) {
    state.thread = payload.thread;
  } else if (eventName === "turn.started" || eventName === "turn.completed") {
    if (payload.turn) upsertTurn(state, payload.turn);
    if (eventName === "turn.completed") {
      clearTurnAttention(state, envelope.turn_id || payload.turn?.id || "");
    }
  } else if (eventName === "turn.lifecycle") {
    const turnId = envelope.turn_id;
    const turn = turnId ? state.turns.get(turnId) : null;
    if (turn && payload.status) {
      state.turns.set(turnId, { ...turn, status: payload.status });
    }
  } else if (eventName === "turn.interrupt_requested") {
    const turnId = envelope.turn_id;
    const turn = turnId ? state.turns.get(turnId) : null;
    if (turn) state.turns.set(turnId, { ...turn, status: "in_progress" });
  } else if (
    eventName === "item.started"
    || eventName === "item.completed"
    || eventName === "item.failed"
    || eventName === "item.interrupted"
    || eventName === "agent.spawned"
    || eventName === "agent.progress"
    || eventName === "agent.completed"
    || eventName === "agent.list"
  ) {
    if (payload.item) upsertItem(state, payload.item);
  } else if (eventName === "item.delta") {
    appendItemDelta(state, envelope.item_id, payload);
  } else if (eventName === "approval.required") {
    const approvalId = payload.approval_id || payload.id;
    if (approvalId) {
      state.approvals.set(approvalId, {
        ...payload,
        turn_id: payload.turn_id || envelope.turn_id || "",
      });
    }
  } else if (eventName === "approval.decided" || eventName === "approval.timeout") {
    const approvalId = payload.approval_id || payload.id;
    if (approvalId) state.approvals.delete(approvalId);
  } else if (eventName === "user_input.required") {
    const inputId = payload.id;
    if (inputId) {
      state.userInputs.set(inputId, {
        ...payload,
        turn_id: payload.turn_id || envelope.turn_id || "",
      });
    }
  } else if (eventName === "user_input.answered" || eventName === "user_input.canceled") {
    const inputId = payload.input_id || payload.id;
    if (inputId) state.userInputs.delete(inputId);
  } else if (eventName === "tool_call.requested") {
    if (payload.call_id) {
      state.dynamicToolCalls.set(payload.call_id, {
        ...payload,
        turn_id: payload.turn_id || envelope.turn_id || "",
      });
    }
  } else if (
    eventName === "tool_call.resolved"
    || eventName === "tool_call.canceled"
    || eventName === "tool_call.timeout"
  ) {
    if (payload.call_id) state.dynamicToolCalls.delete(payload.call_id);
  }
  return true;
}

function clearTurnAttention(state, turnId) {
  for (const [id, approval] of state.approvals) {
    if (!approval?.turn_id || approval.turn_id === turnId) state.approvals.delete(id);
  }
  for (const [id, input] of state.userInputs) {
    if (!input?.turn_id || input.turn_id === turnId) state.userInputs.delete(id);
  }
  for (const [id, call] of state.dynamicToolCalls) {
    if (!call?.turn_id || call.turn_id === turnId) state.dynamicToolCalls.delete(id);
  }
}

export function runtimeEventContinuity(state, envelope) {
  if (!envelope || envelope.thread_id !== state.threadId) {
    return "ignore";
  }
  const sequence = normalizedSequence(envelope.seq);
  if (sequence <= state.latestSeq) {
    return "ignore";
  }
  if (Object.hasOwn(envelope, "previous_seq")) {
    const previousSequence = normalizedSequence(envelope.previous_seq);
    if (previousSequence !== state.latestSeq) {
      return "gap";
    }
  }
  return "next";
}

export async function snapshotThenSubscribe({
  state,
  threadId,
  loadSnapshot,
  subscribe,
  isCurrent = () => true,
}) {
  const detail = await loadSnapshot(threadId);
  if (!isCurrent() || !applySnapshot(state, detail, threadId)) {
    return false;
  }
  if (!isCurrent()) return false;
  subscribe(threadId, state.latestSeq);
  return true;
}

// ---------------------------------------------------------------------------
// Typed selection identity (#4397)
//
// The dashboard can have two very different things selected: a *saved session*
// (a recording on disk) or a *live thread* (a running runtime object). Almost
// every safety rule in this slice reduces to "which one is it?", so the answer
// is a typed value rather than a pair of loosely-related string fields that can
// both be set, both be empty, or disagree.
// ---------------------------------------------------------------------------

// Nothing selected.
export const NO_TARGET = Object.freeze({ kind: "none" });

// A saved session: read-only. Peek only, never reply, never approve.
export function sessionTarget(sessionId) {
  return Object.freeze({ kind: "session", sessionId: String(sessionId || "") });
}

// A live thread: the only thing that can receive a reply or an approval.
export function threadTarget(threadId) {
  return Object.freeze({ kind: "thread", threadId: String(threadId || "") });
}

// May the composer send to this target?
//
// Only a live thread. A saved session has no runtime to receive a message;
// offering a composer against one would be an affordance with nothing behind
// it, and "resume it silently on send" would attach the user's message to a
// thread they never asked to create.
export function canReply(target) {
  return target?.kind === "thread" && Boolean(target.threadId);
}

// Resolve the id a reply must be POSTed to, or an explicit refusal.
//
// Fails closed on every ambiguity: no target, a session target, or a target
// whose thread is not the one the live stream is following (a stale target —
// the user changed rows while a request was in flight).
export function resolveReplyTarget(target, streamState) {
  if (!target || target.kind === "none") {
    return { ok: false, reason: "no-target" };
  }
  if (target.kind === "session") {
    return { ok: false, reason: "session-not-live" };
  }
  if (!target.threadId) {
    return { ok: false, reason: "no-target" };
  }
  if (streamState && streamState.threadId && streamState.threadId !== target.threadId) {
    return { ok: false, reason: "stale-target" };
  }
  return { ok: true, threadId: target.threadId };
}

// Resolve an approval decision to the thread that owns it, or refuse.
//
// An approval is authority: answering the wrong one, or answering one that
// has already been decided elsewhere, is worse than not answering. So the
// approval must be present in the *current* stream state, and that state must
// belong to the selected live thread.
export function resolveApprovalTarget(approvalId, target, streamState) {
  const reply = resolveReplyTarget(target, streamState);
  if (!reply.ok) return reply;
  if (!approvalId) return { ok: false, reason: "no-approval" };
  if (!streamState || streamState.threadId !== reply.threadId) {
    return { ok: false, reason: "stale-target" };
  }
  if (!streamState.approvals || !streamState.approvals.has(approvalId)) {
    // Decided, timed out, or belonging to a thread we are no longer watching.
    return { ok: false, reason: "stale-approval" };
  }
  return { ok: true, threadId: reply.threadId, approvalId };
}

// Resolve a pending user-input submission to the live thread that owns it.
export function resolveUserInputTarget(inputId, target, streamState) {
  const reply = resolveReplyTarget(target, streamState);
  if (!reply.ok) return reply;
  if (!inputId) return { ok: false, reason: "no-user-input" };
  if (!streamState || streamState.threadId !== reply.threadId) {
    return { ok: false, reason: "stale-target" };
  }
  if (!streamState.userInputs || !streamState.userInputs.has(inputId)) {
    return { ok: false, reason: "stale-user-input" };
  }
  return { ok: true, threadId: reply.threadId, inputId };
}

/**
 * Collect answers with TUI parity: Other is always available, and each
 * question yields the exact answer cardinality the control implies
 * (one for single-select; one-or-more for multi-select).
 */
export function collectUserInputAnswers(groups) {
  const answers = [];
  for (const group of groups) {
    const selected = [];
    for (const control of group.controls) {
      if (control.input?.checked) {
        selected.push({
          id: group.question.id,
          label: control.label,
          value: control.value,
        });
      }
    }
    const otherValue = group.other?.value?.trim?.() || "";
    if (otherValue) {
      selected.push({ id: group.question.id, label: "Other", value: otherValue });
    }
    if (selected.length === 0) {
      return {
        ok: false,
        reason: `Choose an answer for ${group.question.header || group.question.question || "each question"}.`,
      };
    }
    if (!group.question.multi_select && selected.length !== 1) {
      return {
        ok: false,
        reason: `Pick exactly one answer for ${group.question.header || group.question.question || "each question"}.`,
      };
    }
    answers.push(...selected);
  }
  return { ok: true, answers };
}

/** Compact preview for long MCP / tool failure receipts. */
export function compactFailureSummary(summary, detail, maxChars = 160) {
  const head = String(summary || "").trim();
  const body = String(detail || "").trim();
  if (!body || body === head) return head || "Failed";
  if (body.length <= maxChars && !looksLikeMcpOrToolFailure(head, body)) {
    return head || body.slice(0, maxChars);
  }
  const preview = (head || body).replace(/\s+/g, " ").slice(0, maxChars);
  return preview.endsWith("…") ? preview : `${preview}…`;
}

export function looksLikeMcpOrToolFailure(summary, detail) {
  const text = `${summary || ""}\n${detail || ""}`.toLowerCase();
  return (
    text.includes("mcp")
    || text.includes("tool failed")
    || text.includes("tool error")
    || text.includes("server error")
    || text.includes("traceback")
    || (detail && detail.length > 280 && /failed|error|exception/i.test(text))
  );
}

/** Capture open disclosures + selection so a stream re-render can restore them. */
export function captureTranscriptChrome(root) {
  const openIds = [];
  const selectedText = globalThis.getSelection?.()?.toString?.() || "";
  if (!root) return { openIds, selectedText, focusItemId: null, focusIsOther: false };
  for (const details of root.querySelectorAll("details[data-item-id][open]")) {
    openIds.push(details.getAttribute("data-item-id"));
  }
  const active = root.ownerDocument?.activeElement;
  let focusItemId = null;
  let focusIsOther = false;
  if (active && root.contains(active)) {
    focusItemId = active.closest?.("[data-item-id]")?.getAttribute("data-item-id") || null;
    focusIsOther = active.classList?.contains("other-answer") || active.tagName === "TEXTAREA";
  }
  return { openIds, selectedText, focusItemId, focusIsOther };
}

export function restoreTranscriptChrome(root, chrome) {
  if (!root || !chrome) return;
  const escape = (value) =>
    (globalThis.CSS && typeof globalThis.CSS.escape === "function")
      ? globalThis.CSS.escape(value)
      : String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
  for (const id of chrome.openIds || []) {
    const details = root.querySelector(`details[data-item-id="${escape(id)}"]`);
    if (details) details.open = true;
  }
}

// Human-readable reason for a refusal, for the status banner.
export function refusalMessage(reason) {
  switch (reason) {
    case "session-not-live":
      return "This is a saved session, not a live thread — nothing was sent. Resume it first to reply.";
    case "stale-target":
      return "That thread is no longer the selected one — nothing was sent.";
    case "stale-approval":
      return "That request was already answered or has expired — nothing was sent.";
    case "stale-user-input":
      return "That question was already answered or has expired — nothing was sent.";
    case "no-approval":
      return "No approval was identified — nothing was sent.";
    case "no-user-input":
      return "No pending question was identified — nothing was sent.";
    case "no-target":
      return "Select a live thread first — nothing was sent.";
    default:
      return "Nothing was sent.";
  }
}

// The SSE resume cursor and whether the stream is known to have a gap.
//
// Surfaced rather than kept internal: after a reconnect the user needs to
// know whether what they are reading is continuous or whether events were
// missed and a re-snapshot is pending.
export function streamCursor(state, { gap = false, connected = true } = {}) {
  const seq = normalizedSequence(state?.latestSeq);
  return {
    latestSeq: seq,
    gap: Boolean(gap),
    connected: Boolean(connected),
    label: !connected
      ? `Reconnecting — resuming from #${seq}`
      : gap
        ? `Gap detected — re-syncing from #${seq}`
        : `Live — event #${seq}`,
  };
}

export function eventStreamUrl(threadId, latestSeq) {
  return `/v1/threads/${encodeURIComponent(threadId)}/events?since_seq=${normalizedSequence(latestSeq)}`;
}

export function saveDraft(drafts, threadId, value) {
  if (!threadId) return;
  if (value) drafts.set(threadId, value);
  else drafts.delete(threadId);
}

export function restoreDraft(drafts, threadId) {
  return drafts.get(threadId) || "";
}

export function setSafeText(element, value) {
  element.textContent = value == null ? "" : String(value);
  return element;
}

function normalizedSequence(value) {
  const sequence = Number(value);
  return Number.isSafeInteger(sequence) && sequence > 0 ? sequence : 0;
}

function upsertTurn(state, turn) {
  if (!turn || !turn.id) return;
  if (!state.turns.has(turn.id)) state.turnOrder.push(turn.id);
  state.turns.set(turn.id, turn);
}

function upsertItem(state, item) {
  if (!item || !item.id) return;
  if (!state.items.has(item.id)) state.itemOrder.push(item.id);
  state.items.set(item.id, item);
}

function appendItemDelta(state, itemId, payload) {
  if (!itemId) return;
  const delta = typeof payload.delta === "string" ? payload.delta : "";
  const existing = state.items.get(itemId) || {
    id: itemId,
    turn_id: "",
    kind: payload.kind || "agent_message",
    status: "in_progress",
    summary: "",
    detail: "",
  };
  if (!state.items.has(itemId)) state.itemOrder.push(itemId);
  state.items.set(itemId, {
    ...existing,
    status: "in_progress",
    detail: `${existing.detail || ""}${delta}`,
  });
}

function startBrowserClient() {
  const dom = {
    shell: document.querySelector("#app-shell"),
    rail: document.querySelector("#thread-rail"),
    session: document.querySelector("main.session"),
    railOpen: document.querySelector("#rail-open"),
    railClose: document.querySelector("#rail-close"),
    railScrim: document.querySelector("#rail-scrim"),
    search: document.querySelector("#thread-search"),
    threadList: document.querySelector("#thread-list"),
    newThread: document.querySelector("#new-thread"),
    connectionDot: document.querySelector("#connection-dot"),
    connectionLabel: document.querySelector("#connection-label"),
    runtimeProvenance: document.querySelector("#runtime-provenance"),
    kicker: document.querySelector("#session-kicker"),
    title: document.querySelector("#session-title"),
    facts: document.querySelector("#session-facts"),
    rename: document.querySelector("#rename-thread"),
    archive: document.querySelector("#archive-thread"),
    status: document.querySelector("#status-banner"),
    transcript: document.querySelector("#transcript"),
    attention: document.querySelector("#attention"),
    composer: document.querySelector("#composer"),
    composerInput: document.querySelector("#composer-input"),
    send: document.querySelector("#send-message"),
    interrupt: document.querySelector("#interrupt-turn"),
    renameDialog: document.querySelector("#rename-dialog"),
    renameForm: document.querySelector("#rename-form"),
    renameInput: document.querySelector("#rename-input"),
    peek: document.querySelector("#session-peek"),
    savedSessions: document.querySelector("#saved-sessions"),
    sessionList: document.querySelector("#session-list"),
  };

  const app = {
    summaries: [],
    sessionSummaries: [],
    // Typed selection: `none`, a read-only `session`, or a live `thread`.
    // Every reply/approval authority check reads this, not a loose id.
    target: NO_TARGET,
    // Bounded, redacted peek for the selected saved session, or null.
    peek: null,
    // Set when the SSE stream reported a sequence gap and a re-snapshot is
    // pending. Surfaced in the connection label rather than hidden.
    streamGap: false,
    selectedThreadId: "",
    threadState: createThreadState(),
    workspace: null,
    runtimeInfo: null,
    drafts: new Map(),
    stream: null,
    reconnectTimer: null,
    generation: 0,
    searchTimer: null,
    railPreviousFocus: null,
    railKeyHandler: null,
  };

  function element(tag, className, text) {
    const created = document.createElement(tag);
    if (className) created.className = className;
    if (text != null) setSafeText(created, text);
    return created;
  }

  function railFocusable() {
    if (!dom.rail) return [];
    return Array.from(
      dom.rail.querySelectorAll(
        'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((node) => node.getClientRects().length > 0);
  }

  function releaseRailModal() {
    if (dom.session) dom.session.inert = false;
    if (dom.rail) {
      dom.rail.removeAttribute("aria-modal");
      dom.rail.removeAttribute("role");
    }
    if (app.railKeyHandler) {
      document.removeEventListener("keydown", app.railKeyHandler, true);
      app.railKeyHandler = null;
    }
  }

  function closeRail() {
    const restore = app.railPreviousFocus;
    app.railPreviousFocus = null;
    releaseRailModal();
    dom.shell.classList.remove("rail-visible");
    const target = restore && typeof restore.focus === "function" ? restore : dom.railOpen;
    target?.focus?.({ preventScroll: true });
  }

  function openRail() {
    if (dom.shell.classList.contains("rail-visible")) return;
    app.railPreviousFocus = document.activeElement;
    dom.shell.classList.add("rail-visible");
    if (dom.session) dom.session.inert = true;
    if (dom.rail) {
      dom.rail.setAttribute("role", "dialog");
      dom.rail.setAttribute("aria-modal", "true");
    }
    app.railKeyHandler = (event) => {
      if (!dom.shell.classList.contains("rail-visible")) return;
      if (event.key === "Escape") {
        event.preventDefault();
        closeRail();
        return;
      }
      if (event.key !== "Tab") return;
      const candidates = railFocusable();
      const first = candidates[0];
      const last = candidates[candidates.length - 1];
      if (!first || !last) {
        event.preventDefault();
        return;
      }
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !dom.rail?.contains(active))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || !dom.rail?.contains(active))) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", app.railKeyHandler, true);
    requestAnimationFrame(() => {
      (railFocusable()[0] || dom.railClose)?.focus?.({ preventScroll: true });
    });
  }

  function syncVisualViewport() {
    const viewport = globalThis.visualViewport;
    const height = viewport?.height || globalThis.innerHeight || 0;
    if (height > 0) {
      document.documentElement.style.setProperty("--vv-height", `${Math.round(height)}px`);
    }
  }

  function setConnection(kind, message) {
    dom.connectionDot.className = `connection-dot ${kind || ""}`.trim();
    setSafeText(dom.connectionLabel, message);
  }

  function showStatus(message) {
    setSafeText(dom.status, message || "");
    dom.status.hidden = !message;
  }

  async function api(path, options = {}) {
    const headers = new Headers(options.headers || {});
    if (options.body != null && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    const response = await fetch(path, {
      ...options,
      headers,
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!response.ok) {
      let message = `${response.status} ${response.statusText}`.trim();
      try {
        const body = await response.json();
        message = body?.error?.message || body?.message || message;
      } catch (_error) {
        // The status line is enough when the response is not JSON.
      }
      if (response.status === 401) {
        message = "This browser session is not authenticated. Restart `codewhale web` to open a fresh one-time session.";
      }
      throw new Error(message);
    }
    if (response.status === 204) return null;
    const contentType = response.headers.get("content-type") || "";
    return contentType.includes("application/json") ? response.json() : response.text();
  }

  function renderThreadList() {
    dom.threadList.replaceChildren();
    if (app.summaries.length === 0) {
      const empty = element("p", "thread-preview", "No matching threads");
      empty.style.padding = "8px 10px";
      dom.threadList.append(empty);
      return;
    }
    for (const summary of app.summaries) {
      const row = element("button", "thread-row");
      row.type = "button";
      row.dataset.threadId = summary.id;
      row.setAttribute("aria-current", summary.id === app.selectedThreadId ? "true" : "false");
      const titleRow = element("span", "thread-title-row");
      titleRow.append(element("span", "thread-title", summary.title || "New thread"));
      const status = element("span", `status-pip ${summary.latest_turn_status === "inprogress" || summary.latest_turn_status === "in_progress" ? "running" : summary.latest_turn_status === "failed" ? "failed" : ""}`);
      status.setAttribute("aria-label", summary.latest_turn_status || "idle");
      titleRow.append(status);
      row.append(titleRow);
      row.append(element("span", "thread-preview", summary.preview || "No messages yet"));
      const branch = summary.branch || basename(summary.workspace) || "local";
      row.append(element("span", "thread-meta", `${branch} · ${relativeTime(summary.updated_at)}`));
      row.addEventListener("click", () => selectThread(summary.id));
      dom.threadList.append(row);
    }
  }

  async function loadThreads(search = dom.search.value.trim()) {
    const query = new URLSearchParams({ limit: "100" });
    if (search) query.set("search", search);
    app.summaries = await api(`/v1/threads/summary?${query.toString()}`);
    renderThreadList();
    return app.summaries;
  }

  // Saved sessions are the durable session store the terminal browses. They
  // are rendered with the same row shape as threads because
  // /v1/sessions/summary and /v1/threads/summary are field-compatible
  // projections — one vocabulary, not two.
  function renderSessionList() {
    dom.sessionList.replaceChildren();
    // The section only exists when the backend actually returned sessions;
    // an affordance for an empty store would imply a capability that has
    // nothing behind it.
    dom.savedSessions.hidden = app.sessionSummaries.length === 0;
    if (app.sessionSummaries.length === 0) return;

    for (const summary of app.sessionSummaries) {
      const row = element("button", "thread-row");
      row.type = "button";
      row.dataset.sessionId = summary.id;
      const titleRow = element("span", "thread-title-row");
      titleRow.append(element("span", "thread-title", summary.title || "Untitled session"));
      row.append(titleRow);
      row.append(element("span", "thread-preview", summary.preview || summary.title));
      const scope = basename(summary.workspace) || "local";
      row.append(
        element(
          "span",
          "thread-meta",
          `${scope} · ${summary.message_count} msg · ${relativeTime(summary.updated_at)}`,
        ),
      );
      row.setAttribute(
        "aria-current",
        app.target.kind === "session" && app.target.sessionId === summary.id ? "true" : "false",
      );
      // Click peeks; resuming is a separate, explicit button inside the peek.
      row.addEventListener("click", () => peekSession(summary.id));
      dom.sessionList.append(row);
    }
  }

  async function loadSessions(search = dom.search.value.trim()) {
    const query = new URLSearchParams({ limit: "50" });
    if (search) query.set("search", search);
    try {
      app.sessionSummaries = await api(`/v1/sessions/summary?${query.toString()}`);
    } catch (_error) {
      // A runtime without a readable session store is not a broken dashboard;
      // hide the section rather than blocking the thread view behind an error.
      app.sessionSummaries = [];
    }
    renderSessionList();
    return app.sessionSummaries;
  }

  // Resume goes through the existing endpoint, which seeds a real thread from
  // the saved messages. The dashboard does not reconstruct history itself.
  // Selecting a saved session shows a read-only peek. It does NOT resume:
  // resuming spawns a real thread and an engine, which must be a deliberate
  // act, not a side effect of clicking a row to see what it was about.
  async function peekSession(sessionId) {
    stopStream();
    app.selectedThreadId = "";
    app.threadState = createThreadState();
    app.target = sessionTarget(sessionId);
    showStatus("");
    renderThreadList();
    renderSessionList();
    try {
      // `?peek=true` returns a bounded, redacted projection — twelve entries,
      // tool payloads summarised — so the browser never receives the full
      // transcript in order to display a preview of it.
      app.peek = await api(
        `/v1/sessions/${encodeURIComponent(sessionId)}?peek=true&entries=12`,
      );
    } catch (error) {
      app.peek = null;
      showStatus(error.message);
    }
    renderAll();
  }

  async function resumeSession(sessionId) {
    showStatus("");
    try {
      const resumed = await api(`/v1/sessions/${encodeURIComponent(sessionId)}/resume-thread`, {
        method: "POST",
        body: "{}",
      });
      app.peek = null;
      await loadThreads("");
      // `selectThread` sets the live thread target; only after this can the
      // composer or an approval act.
      await selectThread(resumed.thread_id);
      showStatus(resumed.summary || "");
    } catch (error) {
      showStatus(error.message);
    }
  }

  // Render the read-only peek pane for a selected saved session.
  function renderPeek() {
    if (!dom.peek) return;
    const showing = app.target.kind === "session" && app.peek;
    dom.peek.hidden = !showing;
    if (!showing) {
      dom.peek.replaceChildren();
      return;
    }
    const peek = app.peek;
    dom.peek.replaceChildren();

    const header = element("div", "peek-header");
    header.append(element("p", "eyebrow", "Saved session — read only"));
    header.append(element("h2", "", peek.title || "Untitled session"));
    header.append(
      element(
        "p",
        "thread-meta",
        `${basename(peek.workspace) || "local"} · ${peek.message_count} messages · ${relativeTime(peek.updated_at)}${peek.archived ? " · archived" : ""}`,
      ),
    );
    dom.peek.append(header);

    if (peek.omitted_before > 0) {
      dom.peek.append(
        element("p", "peek-omitted", `${peek.omitted_before} earlier messages not shown`),
      );
    }

    for (const entry of peek.entries || []) {
      const row = element("div", `peek-entry peek-${entry.kind}`);
      row.append(element("span", "peek-kind", entry.kind));
      // `element()` assigns via textContent. Peek text is recorded user/model
      // content and must never reach an HTML sink; this is the XSS boundary.
      row.append(element("p", "peek-text", entry.text));
      if (entry.redacted) row.append(element("span", "peek-flag", "redacted"));
      if (entry.truncated) row.append(element("span", "peek-flag", "truncated"));
      dom.peek.append(row);
    }

    const resume = element("button", "primary-button", "Resume into a live thread");
    resume.type = "button";
    resume.addEventListener("click", () => resumeSession(peek.session_id));
    dom.peek.append(resume);
  }

  function stopStream() {
    if (app.stream) app.stream.close();
    app.stream = null;
    if (app.reconnectTimer) clearTimeout(app.reconnectTimer);
    app.reconnectTimer = null;
  }

  async function selectThread(threadId) {
    if (!threadId) return;
    saveDraft(app.drafts, app.selectedThreadId, dom.composerInput.value);
    stopStream();
    app.selectedThreadId = threadId;
    // A live thread is now the target: from here the composer and approvals
    // may act. Clear any saved-session peek so the two surfaces are exclusive.
    app.target = threadTarget(threadId);
    app.peek = null;
    app.streamGap = false;
    app.threadState = createThreadState(threadId);
    app.generation += 1;
    const generation = app.generation;
    dom.composerInput.value = restoreDraft(app.drafts, threadId);
    resizeComposer();
    renderThreadList();
    renderAll();
    closeRailIfNarrow();
    setConnection("", "Loading thread snapshot…");
    showStatus("");

    try {
      const subscribed = await snapshotThenSubscribe({
        state: app.threadState,
        threadId,
        loadSnapshot: (id) => api(`/v1/threads/${encodeURIComponent(id)}`),
        subscribe: (id, sequence) => connectStream(id, sequence, generation),
        isCurrent: () => generation === app.generation && threadId === app.selectedThreadId,
      });
      if (!subscribed) return;
      renderAll();
      setConnection("ready", "Local runtime connected");
    } catch (error) {
      if (generation !== app.generation) return;
      showStatus(error.message);
      setConnection("error", "Runtime connection failed");
    }
  }

  function connectStream(threadId, sequence, generation) {
    if (generation !== app.generation || threadId !== app.selectedThreadId) return;
    if (app.stream) app.stream.close();
    const stream = new EventSource(eventStreamUrl(threadId, sequence), { withCredentials: true });
    app.stream = stream;
    stream.onopen = () => setConnection("ready", "Local runtime connected");
    const receive = (message) => {
      if (
        app.stream !== stream
        || generation !== app.generation
        || threadId !== app.selectedThreadId
      ) return;
      try {
        const envelope = JSON.parse(message.data);
        if (runtimeEventContinuity(app.threadState, envelope) === "gap") {
          app.streamGap = true;
          renderStreamCursor();
          showStatus("Runtime event continuity changed; refreshing the thread snapshot…");
          void recoverProjection(threadId, generation, stream);
          return;
        }
        if (!applyRuntimeEvent(app.threadState, envelope)) return;
        renderAll(true);
        if (envelope.event === "turn.completed" || envelope.event === "thread.updated") {
          loadThreads().catch((error) => showStatus(error.message));
        }
      } catch (error) {
        showStatus(`Could not read a Runtime event: ${error.message}`);
      }
    };
    for (const name of STREAM_EVENT_NAMES) stream.addEventListener(name, receive);
    stream.onerror = () => {
      if (app.stream !== stream) {
        stream.close();
        return;
      }
      stream.close();
      app.stream = null;
      if (generation !== app.generation || threadId !== app.selectedThreadId) return;
      setConnection("", "Reconnecting to local runtime…");
      app.reconnectTimer = setTimeout(
        () => connectStream(threadId, app.threadState.latestSeq, generation),
        900,
      );
    };
  }

  async function recoverProjection(threadId, generation, sourceStream = null) {
    if (
      generation !== app.generation
      || threadId !== app.selectedThreadId
      || (sourceStream && app.stream !== sourceStream)
    ) return;

    if (app.stream) app.stream.close();
    app.stream = null;
    if (app.reconnectTimer) clearTimeout(app.reconnectTimer);
    app.reconnectTimer = null;
    setConnection("", "Refreshing thread snapshot…");

    try {
      const subscribed = await snapshotThenSubscribe({
        state: app.threadState,
        threadId,
        loadSnapshot: (id) => api(`/v1/threads/${encodeURIComponent(id)}`),
        subscribe: (id, sequence) => connectStream(id, sequence, generation),
        isCurrent: () => generation === app.generation && threadId === app.selectedThreadId,
      });
      if (!subscribed) return;
      // Gap state clears only after both the replacement snapshot and the
      // resubscription have succeeded.
      app.streamGap = false;
      renderAll();
      showStatus("");
      setConnection("ready", "Local runtime connected");
    } catch (error) {
      if (generation !== app.generation || threadId !== app.selectedThreadId) return;
      // Leave streamGap true so the connection label stays honest until a
      // later recovery actually lands a snapshot + subscription.
      showStatus(`Could not refresh the thread snapshot: ${error.message}`);
      setConnection("error", "Runtime recovery failed");
      app.reconnectTimer = setTimeout(
        () => recoverProjection(threadId, generation),
        900,
      );
    }
  }

  function renderAll(preserveScroll = false) {
    renderHeader();
    renderPeek();
    renderTranscript(preserveScroll);
    renderAttention();
    renderComposer();
    renderThreadList();
    renderSessionList();
    renderStreamCursor();
  }

  // Show the SSE resume cursor so "am I reading everything?" is answerable.
  function renderStreamCursor() {
    if (app.target.kind !== "thread") return;
    const cursor = streamCursor(app.threadState, {
      gap: app.streamGap,
      connected: Boolean(app.stream),
    });
    setConnection(cursor.gap ? "error" : cursor.connected ? "ready" : "", cursor.label);
  }

  function renderHeader() {
    const thread = app.threadState.thread;
    const summary = app.summaries.find((item) => item.id === app.selectedThreadId);
    const title = thread?.title || summary?.title || (thread ? "New thread" : "Choose a thread");
    setSafeText(dom.title, title);
    setSafeText(dom.kicker, thread ? "Local Runtime thread" : "Local Runtime");
    dom.rename.disabled = !thread;
    dom.archive.disabled = !thread;
    dom.facts.replaceChildren();
    if (!thread) return;

    const workspace = summary?.workspace || thread.workspace || app.workspace?.workspace;
    const branch = summary?.branch || app.workspace?.branch;
    dom.facts.append(factChip("Workspace", basename(workspace) || "local"));
    if (branch) dom.facts.append(factChip("Branch", branch));
    dom.facts.append(factChip("Model", thread.model || "Runtime default"));
    dom.facts.append(factChip("Mode", modeLabel(thread.mode)));
    dom.facts.append(factChip("Permission", permissionLabel(thread)));
  }

  function factChip(label, value) {
    const chip = element("span", "fact-chip");
    chip.append(element("span", "", label));
    chip.append(element("strong", "", value));
    return chip;
  }

  function renderTranscript(preserveScroll) {
    const wasNearBottom = dom.transcript.scrollHeight - dom.transcript.scrollTop - dom.transcript.clientHeight < 120;
    const chrome = captureTranscriptChrome(dom.transcript);
    const liveBodies = new Map();
    for (const node of dom.transcript.querySelectorAll("[data-item-id][data-live-body='true']")) {
      liveBodies.set(node.getAttribute("data-item-id"), node);
    }

    // Prefer in-place body patches for growing agent text so assistive tech
    // and selection are not reset on every token.
    let patchedOnly = false;
    if (
      preserveScroll
      && liveBodies.size > 0
      && app.threadState.itemOrder.length > 0
      && dom.transcript.childElementCount === app.threadState.itemOrder.length
    ) {
      patchedOnly = true;
      for (const itemId of app.threadState.itemOrder) {
        const item = app.threadState.items.get(itemId);
        const existing = liveBodies.get(itemId);
        if (!item || !existing || item.kind !== "agent_message" || item.status !== "in_progress") {
          patchedOnly = false;
          break;
        }
        const detail = item.detail || item.summary || "";
        if (existing.textContent !== detail) setSafeText(existing, detail);
      }
    }

    if (!patchedOnly) {
      dom.transcript.replaceChildren();
      if (!app.threadState.thread) {
        dom.transcript.append(emptyState("Your local agent, in the browser.", "Create a thread or choose one from the rail. This client uses the same Runtime as the terminal."));
      } else if (app.threadState.itemOrder.length === 0) {
        dom.transcript.append(emptyState("Ready for a task.", "Send a message below. Model, mode, and permission posture come from the Runtime and are shown read-only above."));
      } else {
        for (const itemId of app.threadState.itemOrder) {
          const item = app.threadState.items.get(itemId);
          if (!item) continue;
          dom.transcript.append(renderItem(item));
        }
        restoreTranscriptChrome(dom.transcript, chrome);
      }
    }
    if (!preserveScroll || wasNearBottom) {
      requestAnimationFrame(() => {
        dom.transcript.scrollTop = dom.transcript.scrollHeight;
      });
    }
  }

  function emptyState(title, description) {
    const empty = element("div", "empty-state");
    empty.append(element("div", "empty-orbit", "◌"));
    empty.append(element("h2", "", title));
    empty.append(element("p", "", description));
    return empty;
  }

  function renderItem(item) {
    const detail = item.detail || item.summary || "";
    if (item.kind === "user_message" || item.kind === "agent_message") {
      const role = item.kind === "user_message" ? "user" : "agent";
      const card = element("article", `message ${role} ${item.status === "in_progress" ? "in-progress" : ""}`.trim());
      card.dataset.itemId = item.id;
      card.append(element("div", "message-label", role === "user" ? "You" : "Codewhale"));
      const body = element("div", "message-body", detail);
      body.dataset.itemId = item.id;
      if (role === "agent" && item.status === "in_progress") {
        // Growing token streams must not re-announce the whole reply.
        body.dataset.liveBody = "true";
        body.setAttribute("aria-live", "off");
      }
      card.append(body);
      return card;
    }
    if (item.kind === "agent_reasoning") {
      const reasoning = element("article", "reasoning");
      reasoning.dataset.itemId = item.id;
      const disclosure = element("details");
      disclosure.dataset.itemId = item.id;
      disclosure.append(element("summary", "", item.status === "in_progress" ? "Reasoning…" : "Reasoning"));
      disclosure.append(element("pre", "", detail));
      reasoning.append(disclosure);
      return reasoning;
    }

    const failed = item.status === "failed";
    const receipt = element("article", `receipt ${failed ? "failed" : ""}`.trim());
    receipt.dataset.itemId = item.id;
    receipt.append(element("div", "receipt-label", `${humanize(item.kind)} · ${humanize(item.status)}`));
    const longFailure = failed && looksLikeMcpOrToolFailure(item.summary, detail);
    if (longFailure) {
      receipt.append(
        element("div", "receipt-summary", compactFailureSummary(item.summary, detail)),
      );
      const disclosure = element("details");
      disclosure.dataset.itemId = item.id;
      disclosure.append(element("summary", "", "Show full error"));
      disclosure.append(element("pre", "", detail || item.summary || ""));
      receipt.append(disclosure);
      return receipt;
    }
    receipt.append(element("div", "receipt-summary", item.summary || detail || humanize(item.kind)));
    if (detail && detail !== item.summary) {
      const disclosure = element("details");
      disclosure.dataset.itemId = item.id;
      disclosure.append(element("summary", "", "Show receipt"));
      disclosure.append(element("pre", "", detail));
      receipt.append(disclosure);
    }
    return receipt;
  }

  function renderAttention() {
    dom.attention.replaceChildren();
    for (const [approvalId, approval] of app.threadState.approvals) {
      dom.attention.append(renderApproval(approvalId, approval));
    }
    for (const [inputId, input] of app.threadState.userInputs) {
      dom.attention.append(renderUserInput(inputId, input));
    }
    dom.attention.hidden = dom.attention.childElementCount === 0;
  }

  function renderApproval(approvalId, approval) {
    const card = element("article", "attention-card");
    card.append(element("p", "eyebrow", "Approval required"));
    card.append(element("h2", "", approval.tool_name || "Tool request"));
    card.append(element("p", "", approval.intent_summary || approval.description || "Codewhale is waiting for permission."));
    const actions = element("div", "attention-actions");
    const rememberLabel = element("label", "remember-field");
    const remember = document.createElement("input");
    remember.type = "checkbox";
    rememberLabel.append(remember, document.createTextNode("Remember for this thread"));
    const deny = element("button", "quiet-button danger", "Deny");
    deny.type = "button";
    deny.addEventListener("click", () => resolveApproval(approvalId, "deny", remember.checked));
    const allow = element("button", "primary-button", "Allow");
    allow.type = "button";
    allow.addEventListener("click", () => resolveApproval(approvalId, "allow", remember.checked));
    actions.append(rememberLabel, deny, allow);
    card.append(actions);
    return card;
  }

  async function resolveApproval(approvalId, decision, remember) {
    // Authority check before authority action. The approval must belong to the
    // thread we are actually watching, and that thread must be the selected
    // live target — never a saved-session peek, never a row the user has since
    // moved off. Refusals are loud and send nothing.
    const resolved = resolveApprovalTarget(approvalId, app.target, app.threadState);
    if (!resolved.ok) {
      showStatus(refusalMessage(resolved.reason));
      renderAttention();
      return;
    }
    try {
      await api(`/v1/approvals/${encodeURIComponent(resolved.approvalId)}`, {
        method: "POST",
        body: JSON.stringify({ decision, remember }),
      });
      app.threadState.approvals.delete(approvalId);
      renderAttention();
    } catch (error) {
      showStatus(error.message);
    }
  }

  function renderUserInput(inputId, envelope) {
    const card = element("form", "attention-card");
    card.dataset.inputId = inputId;
    card.append(element("p", "eyebrow", "Input required"));
    card.append(element("h2", "", "Codewhale has a question"));
    const questions = Array.isArray(envelope.request?.questions) ? envelope.request.questions : [];
    const groups = [];
    for (const question of questions) {
      const fieldset = element("fieldset", "question-fieldset");
      fieldset.append(element("legend", "", question.question || question.header || "Choose an option"));
      const controls = [];
      for (const option of Array.isArray(question.options) ? question.options : []) {
        const label = element("label", "answer-option");
        const input = document.createElement("input");
        input.type = question.multi_select ? "checkbox" : "radio";
        input.name = `question-${inputId}-${question.id}`;
        input.value = option.label || "";
        label.append(input);
        const copy = element("span", "", option.label || "Option");
        if (option.description) copy.append(element("small", "", option.description));
        label.append(copy);
        fieldset.append(label);
        controls.push({ input, label: option.label || "", value: option.label || "" });
      }
      // TUI parity: Other is always reachable, even when allow_free_text is false.
      const other = document.createElement("input");
      other.className = "other-answer";
      other.type = "text";
      other.placeholder = "Other response";
      other.setAttribute("aria-label", `${question.header || "Question"} other response`);
      fieldset.append(other);
      card.append(fieldset);
      groups.push({ question, controls, other });
    }
    const actions = element("div", "attention-actions");
    const submit = element("button", "primary-button", "Submit answers");
    submit.type = "submit";
    actions.append(submit);
    card.append(actions);
    card.addEventListener("submit", async (event) => {
      event.preventDefault();
      const resolved = resolveUserInputTarget(inputId, app.target, app.threadState);
      if (!resolved.ok) {
        showStatus(refusalMessage(resolved.reason));
        renderAttention();
        return;
      }
      const collected = collectUserInputAnswers(groups);
      if (!collected.ok) {
        showStatus(collected.reason);
        return;
      }
      try {
        await api(
          `/v1/user-input/${encodeURIComponent(resolved.threadId)}/${encodeURIComponent(resolved.inputId)}`,
          {
            method: "POST",
            body: JSON.stringify({ answers: collected.answers }),
          },
        );
        app.threadState.userInputs.delete(inputId);
        showStatus("");
        renderAttention();
      } catch (error) {
        showStatus(error.message);
      }
    });
    return card;
  }

  function latestTurn() {
    const id = app.threadState.turnOrder.at(-1);
    return id ? app.threadState.turns.get(id) : null;
  }

  function activeTurn() {
    const turn = latestTurn();
    return turn && (turn.status === "in_progress" || turn.status === "queued") ? turn : null;
  }

  function renderComposer() {
    const ready = Boolean(app.threadState.thread);
    const active = activeTurn();
    dom.composerInput.disabled = !ready;
    dom.send.disabled = !ready || !dom.composerInput.value.trim();
    dom.interrupt.hidden = !active;
    setSafeText(dom.send, active ? "Steer" : "Send");
  }

  async function createThread() {
    showStatus("");
    try {
      const thread = await api("/v1/threads", { method: "POST", body: "{}" });
      await loadThreads("");
      await selectThread(thread.id);
      dom.composerInput.focus();
      return thread;
    } catch (error) {
      showStatus(error.message);
      return null;
    }
  }

  async function sendMessage() {
    const prompt = dom.composerInput.value.trim();
    if (!prompt) return;
    // A reply goes to a live thread or nowhere. A saved-session peek must not
    // silently resume-and-send: that would attach the user's message to a
    // thread they never asked to create.
    if (app.target.kind === "session") {
      showStatus(refusalMessage("session-not-live"));
      return;
    }
    let threadId = app.selectedThreadId;
    if (!threadId) {
      const thread = await createThread();
      if (!thread) return;
      threadId = thread.id;
    }
    const resolved = resolveReplyTarget(threadTarget(threadId), app.threadState);
    if (!resolved.ok) {
      showStatus(refusalMessage(resolved.reason));
      return;
    }
    threadId = resolved.threadId;
    const turn = activeTurn();
    dom.send.disabled = true;
    showStatus("");
    try {
      if (turn) {
        await api(`/v1/threads/${encodeURIComponent(threadId)}/turns/${encodeURIComponent(turn.id)}/steer`, {
          method: "POST",
          body: JSON.stringify({ prompt }),
        });
      } else {
        await api(`/v1/threads/${encodeURIComponent(threadId)}/turns`, {
          method: "POST",
          body: JSON.stringify({ prompt }),
        });
      }
      saveDraft(app.drafts, threadId, "");
      dom.composerInput.value = "";
      resizeComposer();
      renderComposer();
      loadThreads().catch((error) => showStatus(error.message));
    } catch (error) {
      showStatus(error.message);
      renderComposer();
    }
  }

  async function interruptTurn() {
    const turn = activeTurn();
    if (!turn || !app.selectedThreadId) return;
    dom.interrupt.disabled = true;
    try {
      await api(`/v1/threads/${encodeURIComponent(app.selectedThreadId)}/turns/${encodeURIComponent(turn.id)}/interrupt`, { method: "POST" });
    } catch (error) {
      showStatus(error.message);
    } finally {
      dom.interrupt.disabled = false;
    }
  }

  async function archiveThread() {
    if (!app.selectedThreadId) return;
    if (!globalThis.confirm("Archive this thread? You can still access it through the Runtime API.")) return;
    try {
      await api(`/v1/threads/${encodeURIComponent(app.selectedThreadId)}`, {
        method: "PATCH",
        body: JSON.stringify({ archived: true }),
      });
      saveDraft(app.drafts, app.selectedThreadId, "");
      stopStream();
      app.selectedThreadId = "";
      app.threadState = createThreadState();
      await loadThreads();
      if (app.summaries[0]) await selectThread(app.summaries[0].id);
      else renderAll();
    } catch (error) {
      showStatus(error.message);
    }
  }

  function openRenameDialog() {
    if (!app.threadState.thread) return;
    dom.renameInput.value = app.threadState.thread.title || "";
    dom.renameDialog.showModal();
    dom.renameInput.focus();
    dom.renameInput.select();
  }

  async function submitRename(event) {
    event.preventDefault();
    const action = event.submitter?.value;
    if (action !== "save") {
      dom.renameDialog.close();
      return;
    }
    const title = dom.renameInput.value.trim();
    if (!title || !app.selectedThreadId) return;
    try {
      const thread = await api(`/v1/threads/${encodeURIComponent(app.selectedThreadId)}`, {
        method: "PATCH",
        body: JSON.stringify({ title }),
      });
      app.threadState.thread = thread;
      dom.renameDialog.close();
      await loadThreads();
      renderHeader();
    } catch (error) {
      showStatus(error.message);
    }
  }

  function resizeComposer() {
    dom.composerInput.style.height = "auto";
    dom.composerInput.style.height = `${Math.min(dom.composerInput.scrollHeight, 180)}px`;
  }

  function closeRailIfNarrow() {
    if (globalThis.matchMedia("(max-width: 800px)").matches) closeRail();
  }

  dom.railOpen.addEventListener("click", openRail);
  dom.railClose.addEventListener("click", closeRail);
  dom.railScrim.addEventListener("click", closeRail);
  syncVisualViewport();
  globalThis.visualViewport?.addEventListener("resize", syncVisualViewport);
  globalThis.visualViewport?.addEventListener("scroll", syncVisualViewport);
  globalThis.addEventListener("resize", syncVisualViewport);
  dom.newThread.addEventListener("click", createThread);
  dom.rename.addEventListener("click", openRenameDialog);
  dom.archive.addEventListener("click", archiveThread);
  dom.renameForm.addEventListener("submit", submitRename);
  dom.interrupt.addEventListener("click", interruptTurn);
  dom.composer.addEventListener("submit", (event) => {
    event.preventDefault();
    sendMessage();
  });
  dom.composerInput.addEventListener("input", () => {
    saveDraft(app.drafts, app.selectedThreadId, dom.composerInput.value);
    resizeComposer();
    renderComposer();
  });
  dom.composerInput.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      sendMessage();
    }
  });
  dom.search.addEventListener("input", () => {
    if (app.searchTimer) clearTimeout(app.searchTimer);
    app.searchTimer = setTimeout(() => {
      loadThreads().catch((error) => showStatus(error.message));
      loadSessions().catch(() => {});
    }, 180);
  });
  globalThis.addEventListener("beforeunload", stopStream);

  async function initialize() {
    try {
      [app.runtimeInfo, app.workspace] = await Promise.all([
        api("/v1/runtime/info"),
        api("/v1/workspace/status"),
      ]);
      renderRuntimeProvenance(dom.runtimeProvenance, app.runtimeInfo);
      setConnection("ready", "Local runtime connected");
      await loadThreads();
      await loadSessions();
      if (app.summaries[0]) await selectThread(app.summaries[0].id);
      else renderAll();
    } catch (error) {
      setConnection("error", "Runtime connection failed");
      showStatus(error.message);
      renderAll();
    }
  }

  initialize();
}

function basename(path) {
  if (!path) return "";
  const normalized = String(path).replaceAll("\\", "/").replace(/\/$/, "");
  return normalized.split("/").at(-1) || normalized;
}

function humanize(value) {
  if (!value) return "Status";
  return String(value)
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function modeLabel(mode) {
  if (mode === "agent") return "Work";
  if (mode === "plan") return "Plan";
  if (mode === "operate") return "Operate";
  return humanize(mode || "Runtime default");
}

export function formatRuntimeProvenance(runtimeInfo) {
  const version = String(
    runtimeInfo?.codewhale_version || runtimeInfo?.version || "",
  ).trim() || "version unknown";
  const commit = String(runtimeInfo?.codewhale_commit || "").trim();
  const source = /^[0-9a-f]{40}$/i.test(commit)
    ? commit.slice(0, 12)
    : "source unknown";
  return `${version} · ${source}`;
}

export function renderRuntimeProvenance(element, runtimeInfo) {
  return setSafeText(element, formatRuntimeProvenance(runtimeInfo));
}

function permissionLabel(thread) {
  if (thread.trust_mode) return "Full Access";
  if (thread.auto_approve) return "Auto-Review";
  return "Ask";
}

function relativeTime(value) {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "recent";
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}

if (typeof document !== "undefined") {
  startBrowserClient();
}
