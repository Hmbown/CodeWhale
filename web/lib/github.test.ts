import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { lastPageFromLink, relativeTime, fetchFeed } from "./github";

// We test the pure helper functions directly.
// The async fetch functions require mocking the global fetch.

// ── relativeTime ──────────────────────────────────────────────────────

describe("relativeTime", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-01T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns 'just now' for less than 30 seconds ago", () => {
    expect(relativeTime("2026-06-01T11:59:45Z")).toBe("just now");
  });

  it("returns minutes for < 1 hour", () => {
    expect(relativeTime("2026-06-01T11:55:00Z")).toBe("5m");
    expect(relativeTime("2026-06-01T11:30:00Z")).toBe("30m");
  });

  it("returns hours for < 1 day", () => {
    expect(relativeTime("2026-06-01T09:00:00Z")).toBe("3h");
    expect(relativeTime("2026-05-31T18:00:00Z")).toBe("18h");
  });

  it("returns days for < 30 days", () => {
    expect(relativeTime("2026-05-25T12:00:00Z")).toBe("7d");
    expect(relativeTime("2026-05-03T12:00:00Z")).toBe("29d");
  });

  it("returns months for < 12 months", () => {
    expect(relativeTime("2026-03-01T12:00:00Z")).toBe("3mo");
    expect(relativeTime("2025-08-01T12:00:00Z")).toBe("10mo");
  });

  it("returns years for >= 12 months", () => {
    expect(relativeTime("2024-06-01T12:00:00Z")).toBe("2y");
    expect(relativeTime("2025-01-01T00:00:00Z")).toBe("1y");
  });
});

// ── lastPageFromLink (via re-export test) ──────────────────────────────

describe("lastPageFromLink", () => {
  it("returns undefined for null input", () => {
    expect(lastPageFromLink(null)).toBeUndefined();
  });

  it("returns undefined for empty string", () => {
    expect(lastPageFromLink("")).toBeUndefined();
  });

  it("extracts page from Link header with last rel", () => {
    const link =
      '<https://api.github.com/repos/Hmbown/CodeWhale/issues?page=5>; rel="last"';
    expect(lastPageFromLink(link)).toBe(5);
  });

  it("extracts page from multi-part Link header", () => {
    const link = [
      '<https://api.github.com/repos/Hmbown/CodeWhale/issues?page=1>; rel="prev"',
      '<https://api.github.com/repos/Hmbown/CodeWhale/issues?page=3>; rel="last"',
    ].join(", ");
    expect(lastPageFromLink(link)).toBe(3);
  });

  it("returns undefined when no last rel present", () => {
    const link =
      '<https://api.github.com/repos/Hmbown/CodeWhale/issues?page=1>; rel="prev"';
    expect(lastPageFromLink(link)).toBeUndefined();
  });

  it("returns undefined for invalid URL format", () => {
    const link = "not-a-valid-link-header; rel=last";
    expect(lastPageFromLink(link)).toBeUndefined();
  });
});


// ── fetchFeed ────────────────────────────────────────────────────────

describe("fetchFeed", () => {
  let originalFetch: typeof global.fetch;

  beforeEach(() => {
    originalFetch = global.fetch;
    global.fetch = vi.fn();
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.clearAllMocks();
  });

  const mockIssue = {
    number: 1,
    title: "Test Issue",
    html_url: "https://github.com/Hmbown/CodeWhale/issues/1",
    state: "open",
    user: { login: "tester", avatar_url: "https://avatars.githubusercontent.com/u/1?v=4" },
    created_at: "2026-06-01T10:00:00Z",
    updated_at: "2026-06-01T11:00:00Z",
    comments: 0,
    labels: [{ name: "bug", color: "d73a4a" }],
    body: "This is a test issue",
  };

  const mockPullRequestFromIssues = {
    ...mockIssue,
    number: 2,
    title: "Test PR in Issues",
    pull_request: { url: "..." },
    updated_at: "2026-06-01T11:30:00Z",
  };

  const mockPullRequest = {
    number: 3,
    title: "Test PR",
    html_url: "https://github.com/Hmbown/CodeWhale/pull/3",
    state: "open",
    user: { login: "pr-tester", avatar_url: "https://avatars.githubusercontent.com/u/2?v=4" },
    created_at: "2026-06-01T09:00:00Z",
    updated_at: "2026-06-01T10:30:00Z",
    comments: 2,
    labels: [],
    body: "This is a test PR",
    merged_at: null,
    draft: false,
  };

  it("fetches and combines issues and pull requests correctly", async () => {
    const mockIssuesResponse = [mockIssue, mockPullRequestFromIssues];
    const mockPullsResponse = [
      mockPullRequest,
      { ...mockPullRequest, number: 4, state: "closed", merged_at: "2026-06-01T10:00:00Z", updated_at: "2026-06-01T12:00:00Z" },
      { ...mockPullRequest, number: 5, draft: true, updated_at: "2026-06-01T12:30:00Z" }
    ];

    vi.mocked(global.fetch).mockImplementation(async (url) => {
      if (typeof url === "string" && url.includes("/issues")) {
        return { ok: true, json: async () => mockIssuesResponse } as Response;
      }
      if (typeof url === "string" && url.includes("/pulls")) {
        return { ok: true, json: async () => mockPullsResponse } as Response;
      }
      return { ok: false } as Response;
    });

    const feed = await fetchFeed();

    expect(feed.length).toBe(4); // 1 issue + 3 PRs (mockPullRequestFromIssues is filtered)

    // Check sorting (descending by updatedAt)
    expect(feed[0].number).toBe(5); // 12:30
    expect(feed[1].number).toBe(4); // 12:00
    expect(feed[2].number).toBe(1); // 11:00
    expect(feed[3].number).toBe(3); // 10:30

    // Check mapping
    const mergedPr = feed.find(f => f.number === 4);
    expect(mergedPr?.state).toBe("merged");

    const draftPr = feed.find(f => f.number === 5);
    expect(draftPr?.state).toBe("draft");

    const issue = feed.find(f => f.number === 1);
    expect(issue?.kind).toBe("issue");
  });

  it("handles GitHub API errors gracefully", async () => {
    vi.mocked(global.fetch).mockResolvedValue({ ok: false } as Response);

    const feed = await fetchFeed();
    expect(feed).toEqual([]);
  });

  it("passes the correct headers and token to fetch", async () => {
    vi.mocked(global.fetch).mockResolvedValue({ ok: true, json: async () => [] } as Response);

    await fetchFeed("test-token");

    expect(global.fetch).toHaveBeenCalledTimes(2);

    const issuesCall = vi.mocked(global.fetch).mock.calls[0];
    const pullsCall = vi.mocked(global.fetch).mock.calls[1];

    expect(issuesCall[1]?.headers).toHaveProperty("Authorization", "Bearer test-token");
    expect(pullsCall[1]?.headers).toHaveProperty("Authorization", "Bearer test-token");
  });

  it("limits the returned items based on the limit parameter", async () => {
    // Generate 40 items
    const mockIssuesResponse = Array.from({ length: 40 }).map((_, i) => ({
      ...mockIssue,
      number: i + 1,
      updated_at: `2026-06-01T11:${i < 10 ? '0' + i : i}:00Z`,
    }));

    vi.mocked(global.fetch).mockImplementation(async (url) => {
      if (typeof url === "string" && url.includes("/issues")) {
        return { ok: true, json: async () => mockIssuesResponse } as Response;
      }
      if (typeof url === "string" && url.includes("/pulls")) {
        return { ok: true, json: async () => [] } as Response;
      }
      return { ok: false } as Response;
    });

    const feed = await fetchFeed(undefined, 25);
    expect(feed.length).toBe(25);
  });
});
