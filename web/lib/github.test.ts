import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { lastPageFromLink, relativeTime, fetchRepoStats } from "./github";

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


// ── fetchRepoStats ────────────────────────────────────────────────────────

describe("fetchRepoStats", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  const mockDate = new Date("2026-06-01T12:00:00Z");

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(mockDate);
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("fetches repo stats successfully", async () => {
    // We need 4 fetches: repo, contributors, latest release, search PRs
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes("/repos/Hmbown/CodeWhale/contributors")) {
        return {
          ok: true,
          headers: new Headers({
            link: '<https://api.github.com/repositories/123/contributors?per_page=1&anon=true&page=200>; rel="last"',
          }),
          json: async () => [],
        };
      }
      if (url.includes("/repos/Hmbown/CodeWhale/releases/latest")) {
        return {
          ok: true,
          json: async () => ({
            tag_name: "v1.0.0",
            published_at: "2026-05-01T00:00:00Z",
            html_url: "https://github.com/Hmbown/CodeWhale/releases/tag/v1.0.0",
          }),
        };
      }
      if (url.includes("/search/issues")) {
        return {
          ok: true,
          json: async () => ({
            total_count: 5,
          }),
        };
      }
      if (url.includes("/repos/Hmbown/CodeWhale")) {
        return {
          ok: true,
          json: async () => ({
            stargazers_count: 100,
            forks_count: 20,
            open_issues_count: 15,
          }),
        };
      }
      return { ok: false };
    });

    const stats = await fetchRepoStats("fake-token");

    expect(fetchMock).toHaveBeenCalledTimes(4);

    // Check if token is passed correctly
    const calls = fetchMock.mock.calls;
    expect(calls[0][1].headers.Authorization).toBe("Bearer fake-token");
    expect(calls[0][1].headers.Accept).toBe("application/vnd.github+json");

    expect(stats).toEqual({
      stars: 100,
      forks: 20,
      openIssues: 10, // 15 total - 5 PRs
      openPulls: 5,
      contributors: 200,
      latestRelease: {
        tag: "v1.0.0",
        publishedAt: "2026-05-01T00:00:00Z",
        url: "https://github.com/Hmbown/CodeWhale/releases/tag/v1.0.0",
      },
      fetchedAt: mockDate.toISOString(),
    });
  });

  it("handles failed API responses", async () => {
    fetchMock.mockResolvedValue({
      ok: false,
      json: async () => { throw new Error("Should not be called"); },
      headers: new Headers(),
    });

    const stats = await fetchRepoStats();

    expect(stats).toEqual({
      stars: 0,
      forks: 0,
      openIssues: 0,
      openPulls: 0,
      contributors: 141, // MIN_KNOWN_CONTRIBUTORS
      latestRelease: undefined,
      fetchedAt: mockDate.toISOString(),
    });

    // Check no token is passed
    const calls = fetchMock.mock.calls;
    expect(calls[0][1].headers.Authorization).toBeUndefined();
  });

  it("prevents openIssues from going below zero", async () => {
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes("/search/issues")) {
        return {
          ok: true,
          json: async () => ({ total_count: 10 }), // 10 PRs
        };
      }
      if (url.includes("/repos/Hmbown/CodeWhale")) {
        // Excludes contributors and releases strings matching exactly
        if (url.endsWith("/Hmbown/CodeWhale")) {
          return {
            ok: true,
            json: async () => ({ open_issues_count: 5 }), // 5 total issues
          };
        }
      }
      return { ok: false, headers: new Headers() };
    });

    const stats = await fetchRepoStats();
    expect(stats.openIssues).toBe(0); // Math.max(0, 5 - 10)
    expect(stats.openPulls).toBe(10);
  });

  it("extracts contributor count from array length if no link header", async () => {
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes("/contributors")) {
        return {
          ok: true,
          headers: new Headers(),
          json: async () => new Array(150).fill({}), // Array of 150 items
        };
      }
      return { ok: false, headers: new Headers() };
    });

    const stats = await fetchRepoStats();
    expect(stats.contributors).toBe(150);
  });
});
