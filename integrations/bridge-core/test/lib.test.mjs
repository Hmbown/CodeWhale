import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import {
  activeTurnBlock,
  commandAction,
  createRuntimeClient,
  envFirst,
  parseBool,
  parseCommand,
  parseEnvText,
  parseList,
  parseTextContent,
  preservedChatStateFields,
  readJsonSafe,
  readSse,
  splitMessage,
  stripGroupPrefix,
  ThreadStore
} from "../src/lib.mjs";

test("env and primitive parsers handle bridge env conventions", () => {
  assert.equal(envFirst({ A: "", B: " value " }, "A", "B"), "value");
  assert.deepEqual(parseList(" a, b ,, "), ["a", "b"]);
  assert.equal(parseBool("yes"), true);
  assert.equal(parseBool("0", true), false);
  assert.deepEqual(parseEnvText("export A='one'\nB=\"two\"\n# nope"), { A: "one", B: "two" });
  assert.deepEqual(parseEnvText("A='\nB=\"\nEMPTY=\"\""), { A: "'", B: '"', EMPTY: "" });
});

test("parseTextContent supports plain text and JSON text/content wrappers", () => {
  assert.equal(parseTextContent("hello"), "hello");
  assert.equal(parseTextContent(JSON.stringify({ text: "hello" })), "hello");
  assert.equal(parseTextContent(JSON.stringify({ content: "hello" })), "hello");
});

test("stripGroupPrefix supports direct chat types and prefixed group text", () => {
  assert.deepEqual(
    stripGroupPrefix("inspect", {
      chatType: "private",
      requirePrefix: true,
      prefix: "/cw",
      directChatTypes: ["private"]
    }),
    { accepted: true, text: "inspect" }
  );
  assert.deepEqual(
    stripGroupPrefix("/cw inspect", {
      chatType: "group",
      requirePrefix: true,
      prefix: "/cw",
      directChatTypes: ["private"]
    }),
    { accepted: true, text: "inspect" }
  );
});

test("commands map common actions while menu/start stay opt in", () => {
  assert.deepEqual(parseCommand("/allow@CodeWhaleBot ap_1 remember", { stripBotMention: true }), {
    name: "allow",
    args: "ap_1 remember"
  });
  assert.deepEqual(parseCommand("/allow@CodeWhaleBot ap_1 remember"), {
    name: "allow@codewhalebot",
    args: "ap_1 remember"
  });
  assert.deepEqual(commandAction(parseCommand("/status")), { kind: "status" });
  assert.deepEqual(commandAction(parseCommand("/menu")), { kind: "prompt", prompt: "/menu" });
  assert.deepEqual(commandAction(parseCommand("/menu"), { allowMenu: true }), { kind: "menu" });
  assert.deepEqual(commandAction(parseCommand("/start"), { allowStart: true }), { kind: "help" });
});

test("state/message/runtime helpers preserve bridge behavior", () => {
  assert.deepEqual(
    preservedChatStateFields({ model: "m", replyToMessageId: "r", ignored: true }, [
      "model",
      "replyToMessageId"
    ]),
    { model: "m", replyToMessageId: "r" }
  );
  assert.deepEqual(splitMessage("a🧪b", 2), ["a🧪", "b"]);
  assert.deepEqual(splitMessage("alpha beta gamma", 12), ["alpha beta ", "gamma"]);
  const fenced = splitMessage("```js\nconst first = 1;\nconst second = 2;\n```\nDone", 24);
  assert.ok(fenced.length > 1);
  assert.equal(fenced[0].endsWith("\n```"), true);
  assert.equal(fenced[1].startsWith("```js\n"), true);
  assert.equal(fenced.at(-1).includes("Done"), true);
  for (const chunk of fenced) {
    assert.ok(Array.from(chunk).length <= 24);
    assert.equal((chunk.match(/```/g) || []).length % 2, 0);
  }
  assert.deepEqual(activeTurnBlock({ turns: [{ id: "t1", status: "queued" }] }), {
    turnId: "t1",
    message: "Thread already has active turn t1. Wait for it to finish or send /interrupt."
  });
  assert.deepEqual(activeTurnBlock({ turns: [{ status: "in_progress" }] }, null), {
    turnId: "",
    message: "Thread already has active turn (unknown). Wait for it to finish or send /interrupt."
  });
});

test("ThreadStore supports chat state, message dedupe, and action tokens", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "codewhale-bridge-core-"));
  try {
    const statePath = path.join(dir, "thread-map.json");
    const store = await ThreadStore.open(statePath, {
      messageLimit: 2,
      actions: true,
      actionLimit: 2
    });

    await store.setChat("chat-a", { threadId: "thread-a" });
    assert.equal((await store.getChat("chat-a")).threadId, "thread-a");

    assert.equal(await store.recordMessage("m1"), false);
    assert.equal(await store.recordMessage("m1"), true);
    assert.equal(await store.recordMessage("m2"), false);
    assert.equal(await store.recordMessage("m3"), false);
    assert.deepEqual(store.data.messages, ["m2", "m3"]);

    const token = await store.putAction({ kind: "resume", threadId: "thread-a" });
    assert.equal((await store.getAction(token)).kind, "resume");
    assert.equal((await store.takeAction(token)).threadId, "thread-a");
    assert.equal(await store.getAction(token), null);

    const saved = await ThreadStore.open(statePath, { messageLimit: 2, actions: true });
    assert.equal((await saved.getChat("chat-a")).threadId, "thread-a");
    assert.deepEqual(saved.data.messages, ["m2", "m3"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("readJsonSafe tolerates empty and non-JSON bodies", async () => {
  assert.deepEqual(await readJsonSafe({ text: async () => "" }), {});
  assert.deepEqual(await readJsonSafe({ text: async () => '{"ok":true}' }), { ok: true });
  assert.equal(await readJsonSafe({ text: async () => "plain text" }), "plain text");
});

test("readSse reassembles events split across chunks and strips CR", async () => {
  const response = {
    body: (async function* () {
      yield Buffer.from('event: item.delta\ndata: {"seq":1}\n\nevent:');
      yield Buffer.from(' turn.completed\r\ndata: {"seq":2}\n\n');
    })()
  };
  const events = [];
  for await (const event of readSse(response)) events.push(event);
  assert.deepEqual(events, [
    { event: "item.delta", data: '{"seq":1}' },
    { event: "turn.completed", data: '{"seq":2}' }
  ]);
});

test("createRuntimeClient sends bearer auth and surfaces runtime errors", async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options) => {
    calls.push({ url: String(url), options });
    if (String(url).endsWith("/fail")) {
      return {
        ok: false,
        status: 503,
        text: async () => JSON.stringify({ error: { message: "down" } })
      };
    }
    return { ok: true, status: 200, text: async () => JSON.stringify({ ok: true }) };
  };
  try {
    const { runtimeJson, authHeaders } = createRuntimeClient({
      runtimeUrl: "http://127.0.0.1:7878",
      runtimeToken: "token-1"
    });
    assert.deepEqual(authHeaders(), { authorization: "Bearer token-1" });

    assert.deepEqual(await runtimeJson("/v1/threads", { method: "POST", body: { a: 1 } }), {
      ok: true
    });
    assert.equal(calls[0].url, "http://127.0.0.1:7878/v1/threads");
    assert.equal(calls[0].options.method, "POST");
    assert.equal(calls[0].options.headers.authorization, "Bearer token-1");
    assert.equal(calls[0].options.headers["content-type"], "application/json");
    assert.equal(calls[0].options.body, JSON.stringify({ a: 1 }));

    await runtimeJson("/health", { auth: false });
    assert.equal(calls[1].options.method, "GET");
    assert.deepEqual(calls[1].options.headers, {});

    await assert.rejects(() => runtimeJson("/fail"), /Runtime API request failed \(503\): down/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("ThreadStore batches rapid saves into coalesced durable writes", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "codewhale-bridge-core-"));
  try {
    const statePath = path.join(dir, "thread-map.json");
    const store = await ThreadStore.open(statePath);
    let writes = 0;
    const originalWrite = store.writeSnapshot.bind(store);
    store.writeSnapshot = async () => {
      writes += 1;
      return originalWrite();
    };

    await Promise.all(
      Array.from({ length: 25 }, (_, index) =>
        store.setChat(`chat-${index}`, { threadId: `thread-${index}` })
      )
    );
    assert.ok(writes <= 2, `expected coalesced writes, saw ${writes}`);

    const saved = await ThreadStore.open(statePath);
    assert.equal((await saved.getChat("chat-0")).threadId, "thread-0");
    assert.equal((await saved.getChat("chat-24")).threadId, "thread-24");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("ThreadStore persists numeric cursors", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "codewhale-bridge-core-"));
  try {
    const statePath = path.join(dir, "thread-map.json");
    const store = await ThreadStore.open(statePath);

    assert.equal(store.getCursor("telegram.update_offset", 7), 7);
    assert.equal(await store.setCursor("telegram.update_offset", 42), 42);
    assert.equal(store.getCursor("telegram.update_offset"), 42);

    const saved = await ThreadStore.open(statePath);
    assert.equal(saved.getCursor("telegram.update_offset"), 42);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
