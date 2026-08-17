import { describe, expect, it } from "vitest";
import { detectLocaleFromHeaders, matchLocaleTag } from "./detect";

describe("matchLocaleTag", () => {
  it("matches exact full tags case-insensitively", () => {
    expect(matchLocaleTag("pt-BR")).toBe("pt-BR");
    expect(matchLocaleTag("PT-br")).toBe("pt-BR");
    expect(matchLocaleTag("ru")).toBe("ru");
    expect(matchLocaleTag("uk")).toBe("uk");
  });

  it("maps regional variants to the routed base tag", () => {
    expect(matchLocaleTag("ru-RU")).toBe("ru");
    expect(matchLocaleTag("uk-UA")).toBe("uk");
    expect(matchLocaleTag("es-MX")).toBe("es");
    expect(matchLocaleTag("es-419")).toBe("es");
    expect(matchLocaleTag("zh-Hant")).toBe("zh");
    expect(matchLocaleTag("zh-TW")).toBe("zh");
    expect(matchLocaleTag("ja-JP")).toBe("ja");
    expect(matchLocaleTag("ko-KR")).toBe("ko");
    expect(matchLocaleTag("vi-VN")).toBe("vi");
    expect(matchLocaleTag("id-ID")).toBe("id");
  });

  it("routes pt to the only shipped Portuguese variant", () => {
    expect(matchLocaleTag("pt")).toBe("pt-BR");
    expect(matchLocaleTag("pt-PT")).toBe("pt-BR");
  });

  it("matches the routed wave-2 locales through regional variants", () => {
    expect(matchLocaleTag("fr-FR")).toBe("fr");
    expect(matchLocaleTag("fr-CA")).toBe("fr");
    expect(matchLocaleTag("de-DE")).toBe("de");
    expect(matchLocaleTag("de-AT")).toBe("de");
    expect(matchLocaleTag("ca-ES")).toBe("ca");
    expect(matchLocaleTag("hi-IN")).toBe("hi");
    expect(matchLocaleTag("tr-TR")).toBe("tr");
    expect(matchLocaleTag("it-IT")).toBe("it");
    expect(matchLocaleTag("pl-PL")).toBe("pl");
    expect(matchLocaleTag("ar-EG")).toBe("ar");
    expect(matchLocaleTag("ar")).toBe("ar");
  });

  it("rejects unrouted and empty tags deterministically", () => {
    expect(matchLocaleTag("fa")).toBeNull();
    expect(matchLocaleTag("th-TH")).toBeNull();
    expect(matchLocaleTag("nl")).toBeNull();
    expect(matchLocaleTag("")).toBeNull();
    expect(matchLocaleTag("*")).toBeNull();
  });
});

describe("detectLocaleFromHeaders", () => {
  it("prefers an explicit cookie choice over Accept-Language", () => {
    expect(detectLocaleFromHeaders("ru", "ja,en;q=0.8")).toBe("ru");
  });

  it("ignores stale cookies for unrouted locales", () => {
    expect(detectLocaleFromHeaders("th", "uk,en;q=0.8")).toBe("uk");
  });

  it("honors Accept-Language preference order", () => {
    expect(detectLocaleFromHeaders(undefined, "th,vi;q=0.9,ru;q=0.8")).toBe("vi");
    expect(detectLocaleFromHeaders(undefined, "sv,pt;q=0.7")).toBe("pt-BR");
    expect(detectLocaleFromHeaders(undefined, "fr,vi;q=0.9")).toBe("fr");
  });

  it("falls back to the default locale with no signal", () => {
    expect(detectLocaleFromHeaders(undefined, null)).toBe("en");
    expect(detectLocaleFromHeaders(undefined, "")).toBe("en");
    expect(detectLocaleFromHeaders(undefined, "fa,th;q=0.8")).toBe("en");
  });
});
