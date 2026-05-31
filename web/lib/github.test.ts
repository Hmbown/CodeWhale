import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { lastPageFromLink, relativeTime } from "./github";

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

  it("returns days before the 30-day month boundary", () => {
    expect(relativeTime("2026-05-25T12:00:00Z")).toBe("7d");
    expect(relativeTime("2026-05-03T12:00:00Z")).toBe("29d");
  });

  it("returns months for < 12 months", () => {
    expect(relativeTime("2026-05-02T12:00:00Z")).toBe("1mo");
    expect(relativeTime("2026-03-01T12:00:00Z")).toBe("3mo");
    expect(relativeTime("2025-08-01T12:00:00Z")).toBe("10mo");
  });

  it("returns years for >= 12 months", () => {
    expect(relativeTime("2024-06-01T12:00:00Z")).toBe("2y");
    expect(relativeTime("2025-01-01T00:00:00Z")).toBe("1y");
  });
});

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
