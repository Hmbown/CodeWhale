import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const securityMocks = vi.hoisted(() => ({
  agentChat: vi.fn(),
  fetchFeed: vi.fn(),
  getAgentEnv: vi.fn(),
  validateSession: vi.fn(),
}));

vi.mock("@/lib/community-agent", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./community-agent")>();
  return {
    ...actual,
    agentChat: securityMocks.agentChat,
    getAgentEnv: securityMocks.getAgentEnv,
    validateSession: securityMocks.validateSession,
  };
});

vi.mock("@/lib/github", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./github")>();
  return {
    ...actual,
    fetchFeed: securityMocks.fetchFeed,
  };
});

import { POST as adminPost } from "../app/api/admin/post/route";
import { GET as publicFeed } from "../app/api/github/feed/route";
import { runPrReview, runTriage } from "./community-agent-tasks";

class FakeKv {
  readonly values = new Map<string, string>();
  readonly reads: string[] = [];
  readonly deleted: string[] = [];

  async get(key: string): Promise<string | null> {
    this.reads.push(key);
    return this.values.get(key) ?? null;
  }

  async put(key: string, value: string): Promise<void> {
    this.values.set(key, value);
  }

  async list(options?: { prefix?: string; limit?: number }): Promise<{ keys: { name: string }[] }> {
    const prefix = options?.prefix ?? "";
    const limit = options?.limit ?? Number.POSITIVE_INFINITY;
    return {
      keys: [...this.values.keys()]
        .filter((key) => key.startsWith(prefix))
        .slice(0, limit)
        .map((name) => ({ name })),
    };
  }

  async delete(key: string): Promise<void> {
    this.deleted.push(key);
    this.values.delete(key);
  }
}

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function inputUrl(input: string | URL | Request): string {
  if (typeof input === "string") return input;
  return input instanceof URL ? input.toString() : input.url;
}

beforeEach(() => {
  securityMocks.agentChat.mockReset();
  securityMocks.fetchFeed.mockReset();
  securityMocks.getAgentEnv.mockReset();
  securityMocks.validateSession.mockReset();
  securityMocks.validateSession.mockResolvedValue(true);
});

afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

describe("public security boundaries", () => {
  it("rejects a non-draft admin key before any draft KV read or delete", async () => {
    const kv = new FakeKv();
    kv.values.set("dispatch:latest", JSON.stringify({ state: "running" }));
    securityMocks.getAgentEnv.mockResolvedValue({
      CURATED_KV: kv,
      MAINTAINER_TOKEN: "configured",
    });

    const response = await adminPost(new Request("https://codewhale.net/api/admin/post", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        cookie: "mt_sid=test-session",
        origin: "https://codewhale.net",
      },
      body: JSON.stringify({ action: "discard", draftKey: "dispatch:latest" }),
    }));

    await expect(response.json()).resolves.toEqual({ error: "invalid draftKey namespace" });
    expect(response.status).toBe(400);
    expect(securityMocks.validateSession).toHaveBeenCalledOnce();
    expect(kv.reads).toEqual([]);
    expect(kv.deleted).toEqual([]);
    expect(kv.values.has("dispatch:latest")).toBe(true);
  });

  it("never forwards an ambient server token through the public feed route", async () => {
    vi.stubEnv("GITHUB_TOKEN", "server-secret-must-not-cross-public-boundary");
    securityMocks.fetchFeed.mockResolvedValue([]);

    const response = await publicFeed();

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toMatchObject({ items: [] });
    expect(securityMocks.fetchFeed).toHaveBeenCalledExactlyOnceWith(undefined, 50);
  });

  it("does not make another model call for unchanged triage or PR inputs", async () => {
    securityMocks.agentChat.mockResolvedValue({
      content: JSON.stringify({ bodyEn: "review", bodyZh: "审阅" }),
      usage: { input: 10, output: 5 },
    });

    const triageKv = new FakeKv();
    const triageFetch = vi.fn(async (input: string | URL | Request) => {
      const url = inputUrl(input);
      if (!url.includes("/issues?")) throw new Error(`unexpected triage URL: ${url}`);
      return jsonResponse([{
        number: 42,
        title: "Unchanged issue",
        body: "same body",
        updated_at: "2020-01-01T00:00:00.000Z",
        html_url: "https://github.com/Hmbown/CodeWhale/issues/42",
        labels: [],
      }]);
    });
    vi.stubGlobal("fetch", triageFetch);

    const triageEnv = { CURATED_KV: triageKv, DEEPSEEK_API_KEY: "test-key" };
    await expect(runTriage(triageEnv)).resolves.toMatchObject({ processed: 1, skipped: 0 });
    expect(securityMocks.agentChat).toHaveBeenCalledOnce();
    securityMocks.agentChat.mockClear();
    await expect(runTriage(triageEnv)).resolves.toMatchObject({ processed: 0, skipped: 1 });
    expect(securityMocks.agentChat).not.toHaveBeenCalled();
    expect(triageKv.values.has("draft:triage:42")).toBe(true);

    const prKv = new FakeKv();
    const prFetch = vi.fn(async (input: string | URL | Request) => {
      const url = inputUrl(input);
      if (!url.includes("/pulls?")) throw new Error(`unexpected PR URL: ${url}`);
      return jsonResponse([{
        number: 84,
        title: "Unchanged PR",
        body: "same body",
        updated_at: "2020-01-01T00:00:00.000Z",
        html_url: "https://github.com/Hmbown/CodeWhale/pull/84",
        changed_files: 3,
        additions: 10,
        deletions: 2,
        user: { login: "contributor" },
      }]);
    });
    vi.stubGlobal("fetch", prFetch);

    const prEnv = { CURATED_KV: prKv, DEEPSEEK_API_KEY: "test-key" };
    await expect(runPrReview(prEnv)).resolves.toMatchObject({ processed: 1, skipped: 0 });
    expect(securityMocks.agentChat).toHaveBeenCalledOnce();
    securityMocks.agentChat.mockClear();
    await expect(runPrReview(prEnv)).resolves.toMatchObject({ processed: 0, skipped: 1 });
    expect(securityMocks.agentChat).not.toHaveBeenCalled();
    expect(prKv.values.has("draft:pr-review:84")).toBe(true);
  });

  it("posts bodyZh when lang=zh and no editedBody is supplied", async () => {
    const draft = {
      id: "42",
      type: "triage",
      targetNumber: 42,
      bodyEn: "English body",
      bodyZh: "中文正文",
      generatedAt: "2026-01-01T00:00:00.000Z",
      posted: false,
    };
    const kv = new FakeKv();
    kv.values.set("draft:triage:42", JSON.stringify(draft));

    const capturedBodies: string[] = [];
    const mockFetch = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      const url = inputUrl(input);
      if (url.includes("/issues/42/comments")) {
        const reqBody = JSON.parse((init?.body as string) ?? "{}");
        capturedBodies.push(reqBody.body);
        return new Response(JSON.stringify({ id: 1 }), { status: 201, headers: { "content-type": "application/json" } });
      }
      throw new Error(`unexpected URL: ${url}`);
    });
    vi.stubGlobal("fetch", mockFetch);

    securityMocks.getAgentEnv.mockResolvedValue({
      CURATED_KV: kv,
      MAINTAINER_TOKEN: "configured",
      MAINTAINER_GITHUB_PAT: "ghp_test",
      GITHUB_REPO: "Hmbown/CodeWhale",
    });

    const response = await adminPost(new Request("https://codewhale.net/api/admin/post", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        cookie: "mt_sid=test-session",
        origin: "https://codewhale.net",
      },
      body: JSON.stringify({ action: "post", draftKey: "draft:triage:42", lang: "zh" }),
    }));

    await expect(response.json()).resolves.toMatchObject({ ok: true });
    expect(capturedBodies).toHaveLength(1);
    expect(capturedBodies[0]).toBe("中文正文");
  });
});
