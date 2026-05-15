import test from "node:test";
import assert from "node:assert/strict";

import {
  activeTurnBlock,
  commandAction,
  isAllowed,
  pairingRefusalText,
  parseApprovalDecisionArgs,
  parseBool,
  parseEnvText,
  parseCommand,
  parseList,
  parseTextContent,
  splitMessage,
  stripGroupPrefix,
  validateBridgeConfig
} from "../src/lib.mjs";

test("parseList trims empty values", () => {
  assert.deepEqual(parseList(" oc_1, ou_2 ,, "), ["oc_1", "ou_2"]);
});

test("parseBool accepts common truthy values", () => {
  assert.equal(parseBool("yes"), true);
  assert.equal(parseBool("0", true), false);
  assert.equal(parseBool(undefined, true), true);
});

test("parseTextContent reads Feishu JSON text content", () => {
  assert.equal(parseTextContent(JSON.stringify({ text: "hello" })), "hello");
});

test("parseEnvText handles comments, export, and quoted values", () => {
  assert.deepEqual(
    parseEnvText(`
      # ignored
      export FEISHU_DOMAIN="lark"
      DEEPSEEK_WORKSPACE='/opt/whalebro'
    `),
    {
      FEISHU_DOMAIN: "lark",
      DEEPSEEK_WORKSPACE: "/opt/whalebro"
    }
  );
});

test("stripGroupPrefix requires prefix in group chats", () => {
  assert.deepEqual(
    stripGroupPrefix("/ds inspect this", {
      chatType: "group",
      requirePrefix: true,
      prefix: "/ds"
    }),
    { accepted: true, text: "inspect this" }
  );
  assert.equal(
    stripGroupPrefix("inspect this", {
      chatType: "group",
      requirePrefix: true,
      prefix: "/ds"
    }).accepted,
    false
  );
});

test("stripGroupPrefix accepts DM text without group prefix", () => {
  assert.deepEqual(
    stripGroupPrefix("inspect this", {
      chatType: "p2p",
      requirePrefix: true,
      prefix: "/ds"
    }),
    { accepted: true, text: "inspect this" }
  );
});

test("parseCommand distinguishes prompts and slash commands", () => {
  assert.deepEqual(parseCommand("hello"), { name: "prompt", args: "hello" });
  assert.deepEqual(parseCommand("/allow abc remember"), {
    name: "allow",
    args: "abc remember"
  });
});

test("commandAction maps bridge commands and falls back to prompts", () => {
  assert.deepEqual(commandAction(parseCommand("/status")), { kind: "status" });
  assert.deepEqual(commandAction(parseCommand("/resume thread-1")), {
    kind: "resume",
    threadId: "thread-1"
  });
  assert.deepEqual(commandAction(parseCommand("/unknown value")), {
    kind: "prompt",
    prompt: "/unknown value"
  });
});

test("parseApprovalDecisionArgs extracts remember flag", () => {
  assert.deepEqual(parseApprovalDecisionArgs("ap_123 remember"), {
    approvalId: "ap_123",
    remember: true
  });
  assert.deepEqual(parseApprovalDecisionArgs(""), { approvalId: "", remember: false });
});

test("isAllowed checks chat and user identifiers", () => {
  assert.equal(
    isAllowed({ chatId: "oc_x", openId: "ou_y" }, ["ou_y"], false),
    true
  );
  assert.equal(isAllowed({ chatId: "oc_x" }, [], false), false);
  assert.equal(isAllowed({ chatId: "oc_x" }, [], true), true);
});

test("pairingRefusalText includes allowlist identifiers", () => {
  const body = pairingRefusalText({
    chatId: "oc_chat",
    openId: "ou_user",
    unionId: "on_union",
    userId: "u_user"
  });
  assert.match(body, /chat_id=oc_chat/);
  assert.match(body, /open_id=ou_user/);
  assert.match(body, /union_id=on_union/);
  assert.match(body, /user_id=u_user/);
});

test("activeTurnBlock reports active queued or in-progress turn", () => {
  assert.equal(activeTurnBlock({ turns: [{ id: "done", status: "completed" }] }), null);
  assert.deepEqual(
    activeTurnBlock({
      turns: [
        { id: "old", status: "completed" },
        { id: "turn-2", status: "in_progress" }
      ]
    }),
    {
      turnId: "turn-2",
      message: "Thread already has active turn turn-2. Wait for it to finish or send /interrupt."
    }
  );
});

test("splitMessage chunks long text", () => {
  assert.deepEqual(splitMessage("abcdef", 2), ["ab", "cd", "ef"]);
});

test("validateBridgeConfig accepts locked-down whalebro DM config", () => {
  const result = validateBridgeConfig(
    {
      FEISHU_APP_ID: "cli_valid",
      FEISHU_APP_SECRET: "secret",
      FEISHU_DOMAIN: "lark",
      DEEPSEEK_RUNTIME_URL: "http://127.0.0.1:7878",
      DEEPSEEK_RUNTIME_TOKEN: "token-a",
      DEEPSEEK_WORKSPACE: "/opt/whalebro",
      DEEPSEEK_CHAT_ALLOWLIST: "oc_allowed",
      DEEPSEEK_ALLOW_UNLISTED: "false",
      FEISHU_THREAD_MAP_PATH: "/var/lib/deepseek-feishu-bridge/thread-map.json",
      FEISHU_ALLOW_GROUPS: "false",
      FEISHU_REQUIRE_PREFIX_IN_GROUP: "true"
    },
    {
      workspaceRoot: "/opt/whalebro",
      runtimeEnv: {
        DEEPSEEK_RUNTIME_TOKEN: "token-a",
        DEEPSEEK_API_KEY: "sk-valid",
        DEEPSEEK_RUNTIME_PORT: "7878"
      }
    }
  );
  assert.equal(result.ok, true);
  assert.equal(result.errors.length, 0);
});

test("validateBridgeConfig rejects unsafe group pairing and token mismatch", () => {
  const result = validateBridgeConfig(
    {
      FEISHU_APP_ID: "cli_valid",
      FEISHU_APP_SECRET: "secret",
      FEISHU_DOMAIN: "feishu",
      DEEPSEEK_RUNTIME_URL: "http://127.0.0.1:7878",
      DEEPSEEK_RUNTIME_TOKEN: "bridge-token",
      DEEPSEEK_WORKSPACE: "/opt/whalebro",
      DEEPSEEK_ALLOW_UNLISTED: "true",
      FEISHU_THREAD_MAP_PATH: "/var/lib/deepseek-feishu-bridge/thread-map.json",
      FEISHU_ALLOW_GROUPS: "true",
      FEISHU_REQUIRE_PREFIX_IN_GROUP: "false"
    },
    {
      workspaceRoot: "/opt/whalebro",
      runtimeEnv: {
        DEEPSEEK_RUNTIME_TOKEN: "runtime-token",
        DEEPSEEK_API_KEY: "replace-with-deepseek-platform-key"
      }
    }
  );
  assert.equal(result.ok, false);
  assert.match(
    result.errors.map((item) => item.code).join(","),
    /open_group_control/
  );
  assert.match(result.errors.map((item) => item.code).join(","), /token_mismatch/);
  assert.match(result.warnings.map((item) => item.code).join(","), /group_without_prefix/);
});
