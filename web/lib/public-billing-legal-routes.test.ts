import { existsSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { PUBLIC_MEMBERSHIP_COPY, PUBLIC_MEMBERSHIP_STATUS } from "./content/membership";
import { footerLegalLinks } from "./i18n/links";
import { getChrome } from "./i18n/dictionaries";
import { LEGAL_UPDATED, PRIVACY_SECTIONS, TERMS_SECTIONS } from "./legal-copy";

const webRoot = new URL("../", import.meta.url);

describe("marketing pricing and legal routes", () => {
  it("ships real pages for the URLs that used to 404", () => {
    for (const path of [
      "app/[locale]/pricing/page.tsx",
      "app/[locale]/legal/terms/page.tsx",
      "app/[locale]/legal/privacy/page.tsx",
      "app/[locale]/privacy/page.tsx",
      "app/[locale]/terms/page.tsx",
    ]) {
      expect(existsSync(new URL(path, webRoot)), path).toBe(true);
    }
  });

  it("aliases /privacy and /terms onto the legal paths instead of inventing a second policy", () => {
    expect(footerLegalLinks("en", getChrome("en")).map((l) => l.href)).toEqual([
      "/en/pricing",
      "/en/legal/terms",
      "/en/legal/privacy",
    ]);
    expect(LEGAL_UPDATED).toBe("July 23, 2026");
    expect(TERMS_SECTIONS.some((s) => s.title === "Plans and charges")).toBe(true);
    expect(PRIVACY_SECTIONS.some((s) => s.title === "Retention and deletion")).toBe(true);
  });

  it("keeps dormant membership copy free of unapproved commercial terms", () => {
    const copy = JSON.stringify(PUBLIC_MEMBERSHIP_COPY);
    expect(copy).not.toMatch(/\$10|\$50|rollover hosted-compute/i);
    expect(PUBLIC_MEMBERSHIP_STATUS).toEqual({
      checkout: "dormant",
      paymentFromPage: false,
      localUseRequiresPaidMembership: false,
      commercialTerms: "not-published",
    });
  });
});
