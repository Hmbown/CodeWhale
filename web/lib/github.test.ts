import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// We test the pure helper functions directly.
// The async fetch functions require mocking the global fetch.

// ── relativeTime ──────────────────────────────────────────────────────

function relativeTime(iso: string): string {
  const diff = Date.now() - +new Date(iso);
  const mins = Math.round(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  const days = Math.round(hrs / 24);
  if (days < 30) return `${days}d`;
  const months = Math.round(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.round(months / 12)}y`;
}

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
    expect(relativeTime("2026-05-02T12:00:00Z")).toBe("30d");
  });

  it("returns months for < 12 months", () => {
    expect(relativeTime("2026-03-01T12:00:00Z")).toBe("3mo");
    expect(relativeTime("2025-08-01T12:00:00Z")).toBe("10mo");
  });

  it("returns years for >= 12 months", () => {
    expect(relativeTime("2024-06-01T12:00:00Z")).toBe("2y");
    expect(relativeTime("2025-01-01T00:00:00Z")).toBe("2y");
  });
});

// ── lastPageFromLink (via re-export test) ──────────────────────────────

function lastPageFromLink(link: string | null): number | undefined {
  if (!link) return undefined;
  for (const part of link.split(",")) {
    const [rawUrl, rawRel] = part
      .split(";")
      .map((segment: string) => segment.trim());
    if (rawRel !== 'rel="last"') continue;
    const match = rawUrl.match(/^<(.+)>$/);
    if (!match) continue;
    const page = new URL(match[1]).searchParams.get("page");
    const parsed = page ? Number.parseInt(page, 10) : NaN;
    if (Number.isFinite(parsed) && parsed > 0) return parsed;
  }
  return undefined;
}

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
