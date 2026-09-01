#!/usr/bin/env node
/**
 * opencode-chat2responses-proxy.mjs
 *
 * Minimal local proxy that exposes POST /v1/chat/completions (Chat API)
 * but forwards as POST /v1/responses (Responses API) to opencode.ai/zen.
 *
 * Purpose: CodeWhale only spoke Chat Completions, but
 * muse-spark-1.2-contributor-free on https://opencode.ai/zen/v1 only
 * speaks Responses. This shim lets any Chat-only client use that model
 * without modifying Rust code.
 *
 * Usage:
 *   node scripts/opencode-chat2responses-proxy.mjs
 *   # listens on http://127.0.0.1:8765
 *
 *   Then in CodeWhale config.toml:
 *     [providers.my_opencode]
 *     kind = "openai-compatible"
 *     base_url = "http://127.0.0.1:8765/v1"
 *     model = "muse-spark-1.2-contributor-free"
 *     api_key_env = "OPENCODE_ZEN_API_KEY"
 *     # proxy speaks chat to CodeWhale, responses to upstream
 *
 * Prefer the native fix (no proxy needed):
 *     [providers.opencode_zen]
 *     api_key_env = "OPENCODE_ZEN_API_KEY"
 *     base_url = "https://opencode.ai/zen/v1"
 *     model = "muse-spark-1.2-contributor-free"
 *   The bundled offering + resolver now correctly routes muse-spark over
 *   Responses (see crates/config/src/route/offering.rs).
 */

import http from "node:http";

const LISTEN_PORT = Number(process.env.PROXY_PORT ?? 8765);
const UPSTREAM_BASE = process.env.UPSTREAM_BASE ?? "https://opencode.ai/zen/v1";
const UPSTREAM_PATH = "/responses";

function chatToResponses(chatBody) {
  const model = chatBody.model ?? "muse-spark-1.2-contributor-free";
  const messages = chatBody.messages ?? [];
  const tools = chatBody.tools;
  const sysMsgs = messages.filter((m) => m.role === "system");
  const instructions =
    sysMsgs.map((m) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content))).join("\n\n") ||
    "You are a helpful assistant.";
  const input = [];
  for (const m of messages) {
    if (m.role === "system") continue;
    if (m.role === "tool") {
      input.push({
        type: "function_call_output",
        call_id: m.tool_call_id ?? m.toolCallId ?? "call_unknown",
        output: typeof m.content === "string" ? m.content : JSON.stringify(m.content),
      });
      continue;
    }
    const content = typeof m.content === "string" ? [{ type: "input_text", text: m.content }] : m.content;
    if (m.tool_calls || m.toolCalls) {
      for (const tc of m.tool_calls ?? m.toolCalls ?? []) {
        input.push({
          type: "function_call",
          call_id: tc.id,
          name: tc.function?.name ?? tc.name,
          arguments: tc.function?.arguments ?? "{}",
        });
      }
    }
    input.push({
      type: "message",
      role: m.role === "assistant" ? "assistant" : "user",
      content,
    });
  }
  const body = {
    model,
    stream: chatBody.stream ?? false,
    store: false,
    instructions,
    input,
  };
  if (chatBody.max_tokens) body.max_output_tokens = chatBody.max_tokens;
  if (chatBody.temperature != null) body.temperature = chatBody.temperature;
  if (chatBody.top_p != null) body.top_p = chatBody.top_p;
  if (tools) {
    body.tools = tools.map((t) => ({
      type: "function",
      name: t.function.name,
      description: t.function.description ?? "",
      parameters: t.function.parameters ?? { type: "object", properties: {} },
      strict: false,
    }));
    body.tool_choice = "auto";
  }
  return body;
}

function translateResponsesSseToChat(responsesChunk, model) {
  let out = "";
  const lines = responsesChunk.split("\n");
  for (const line of lines) {
    if (!line.startsWith("data:")) continue;
    const payload = line.slice(5).trim();
    if (payload === "[DONE]") {
      out += `data: [DONE]\n\n`;
      continue;
    }
    try {
      const evt = JSON.parse(payload);
      const type = evt.type ?? "";
      if (type === "response.output_text.delta") {
        const delta = evt.delta ?? evt.text ?? "";
        out += `data: ${JSON.stringify({ id: evt.response?.id ?? "chatcmpl-proxy", object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta: { content: delta }, finish_reason: null }] })}\n\n`;
      } else if (type === "response.output_item.added" && evt.item?.type === "function_call") {
        const item = evt.item;
        out += `data: ${JSON.stringify({ id: evt.response?.id ?? "chatcmpl-proxy", object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: item.call_id, type: "function", function: { name: item.name, arguments: "" } }] }, finish_reason: null }] })}\n\n`;
      } else if (type === "response.function_call_arguments.delta") {
        out += `data: ${JSON.stringify({ id: evt.response?.id ?? "chatcmpl-proxy", object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta: { tool_calls: [{ index: 0, function: { arguments: evt.delta ?? "" } }] }, finish_reason: null }] })}\n\n`;
      } else if (type === "response.completed" || type === "response.incomplete") {
        out += `data: ${JSON.stringify({ id: evt.response?.id ?? "chatcmpl-proxy", object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta: {}, finish_reason: "stop" }] })}\n\n`;
      }
    } catch {}
  }
  return out;
}

const server = http.createServer(async (req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, upstream: UPSTREAM_BASE }));
    return;
  }
  if (req.method !== "POST" || !req.url?.includes("/chat/completions")) {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "only POST /v1/chat/completions is proxied" }));
    return;
  }
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", async () => {
    try {
      const chatBody = JSON.parse(body || "{}");
      const model = chatBody.model ?? "muse-spark-1.2-contributor-free";
      const isStream = chatBody.stream === true;
      const apiKey = req.headers.authorization?.replace(/^Bearer\s+/i, "") ?? process.env.OPENCODE_ZEN_API_KEY ?? "";
      const responsesBody = chatToResponses(chatBody);
      const upstreamUrl = `${UPSTREAM_BASE}${UPSTREAM_PATH}`;
      const headers = {
        "content-type": "application/json",
        accept: isStream ? "text/event-stream" : "application/json",
      };
      if (apiKey) headers.authorization = `Bearer ${apiKey}`;
      const upstreamRes = await fetch(upstreamUrl, { method: "POST", headers, body: JSON.stringify(responsesBody) });
      if (!upstreamRes.ok) {
        const text = await upstreamRes.text();
        res.writeHead(upstreamRes.status, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: `upstream ${upstreamRes.status}`, body: text.slice(0, 4000) }));
        return;
      }
      if (!isStream) {
        const data = await upstreamRes.json();
        const outputText = data.output?.flatMap((item) => item.content ?? []).filter((c) => c.type === "output_text").map((c) => c.text).join("") ?? data.output_text ?? "";
        const chatRes = {
          id: data.id ?? "chatcmpl-proxy",
          object: "chat.completion",
          created: Math.floor(Date.now() / 1000),
          model,
          choices: [{ index: 0, message: { role: "assistant", content: outputText }, finish_reason: "stop" }],
          usage: data.usage ? { prompt_tokens: data.usage.input_tokens, completion_tokens: data.usage.output_tokens, total_tokens: (data.usage.input_tokens ?? 0) + (data.usage.output_tokens ?? 0) } : undefined,
        };
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(chatRes));
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive", "x-accel-buffering": "no" });
      const reader = upstreamRes.body.getReader();
      const decoder = new TextDecoder();
      let buf = "";
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buf.indexOf("\n\n")) !== -1) {
          const chunk = buf.slice(0, idx + 2);
          buf = buf.slice(idx + 2);
          const translated = translateResponsesSseToChat(chunk, model);
          if (translated) res.write(translated);
        }
      }
      if (buf.trim()) {
        const translated = translateResponsesSseToChat(buf, model);
        if (translated) res.write(translated);
      }
      res.write(`data: [DONE]\n\n`);
      res.end();
    } catch (e) {
      res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: String(e?.message ?? e).slice(0, 2000) }));
    }
  });
});

server.listen(LISTEN_PORT, "127.0.0.1", () => {
  console.log(`[opencode-proxy] listening on http://127.0.0.1:${LISTEN_PORT}/v1/chat/completions -> ${UPSTREAM_BASE}${UPSTREAM_PATH}`);
  console.log(`[opencode-proxy] health: http://127.0.0.1:${LISTEN_PORT}/health`);
});
