import { describe, expect, it } from "vitest";
import { acceptLanguageTags, detectLocaleFromHeaders, matchLocaleTag } from "./detect";

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

  it("sorts by q weight rather than list position", () => {
    // Header order is not required to be preference order. Reading it
    // positionally handed this reader English.
    expect(detectLocaleFromHeaders(undefined, "en;q=0.2, ja;q=0.9")).toBe("ja");
    expect(detectLocaleFromHeaders(undefined, "en;q=0.3,de;q=0.4,ko;q=0.9")).toBe("ko");
    // An unweighted tag carries the default weight of 1.
    expect(detectLocaleFromHeaders(undefined, "en;q=0.8, uk")).toBe("uk");
  });

  it("drops q=0 tags, which mean 'not acceptable'", () => {
    // RFC 9110 §12.4.2: q=0 refuses the language outright. It used to be
    // treated as merely least-preferred, and being first in the list won.
    expect(detectLocaleFromHeaders(undefined, "en;q=0, ja;q=0.9")).toBe("ja");
    expect(detectLocaleFromHeaders(undefined, "ru;q=0.000")).toBe("en");
  });

  it("keeps original order as the tie-break", () => {
    expect(detectLocaleFromHeaders(undefined, "ru,uk")).toBe("ru");
    expect(detectLocaleFromHeaders(undefined, "ru;q=0.5,uk;q=0.5")).toBe("ru");
    expect(acceptLanguageTags("fr-CA, fr;q=0.9, en;q=0.8")).toEqual([
      "fr-CA",
      "fr",
      "en",
    ]);
  });

  it("leaves a malformed weight at the default rather than guessing", () => {
    expect(acceptLanguageTags("en;q=nonsense, ja;q=0.9")).toEqual(["en", "ja"]);
    expect(acceptLanguageTags("  ,  en  ")).toEqual(["en"]);
  });

  it("falls back to the default locale with no signal", () => {
    expect(detectLocaleFromHeaders(undefined, null)).toBe("en");
    expect(detectLocaleFromHeaders(undefined, "")).toBe("en");
    expect(detectLocaleFromHeaders(undefined, "fa,th;q=0.8")).toBe("en");
  });
});
