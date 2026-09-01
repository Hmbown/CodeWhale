import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { resolveWhale } from "./whale-tokens";

const CSS = readFileSync(new URL("../app/globals.css", import.meta.url), "utf8");

function selectorBlock(selector: string): string {
  const match = CSS.match(new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`, "s"));
  if (!match) throw new Error(`Missing CSS selector: ${selector}`);
  return match[1];
}

function selectorVars(selector: string): Record<string, string> {
  const block = selectorBlock(selector);
  const vars: Record<string, string> = {};
  // Values may be a literal hex or a `var(--whale-*)` reference into the
  // generated app/tokens.css; non-color values (channel triples, lengths) are
  // skipped, exactly as the hex-only regex used to skip them.
  for (const match of block.matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    const value = resolveWhale(match[2].trim());
    if (/^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(value)) vars[match[1]] = value;
  }
  return vars;
}

function relativeLuminance(hex: string): number {
  const full = hex.length === 4 ? hex.slice(1).split("").map((c) => c + c).join("") : hex.slice(1);
  const channels = full
    .match(/.{2}/g)!
    .map((value) => Number.parseInt(value, 16) / 255)
    .map((value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));

  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrastRatio(foreground: string, background: string): number {
  const lighter = Math.max(relativeLuminance(foreground), relativeLuminance(background));
  const darker = Math.min(relativeLuminance(foreground), relativeLuminance(background));
  return (lighter + 0.05) / (darker + 0.05);
}

describe("docs theme contrast contract", () => {
  // Tideline: dark is the site default (the bare `.docs-theme` block inherits
  // the dark surface tokens from `:root`), and the light sheet is the opt-in
  // override. Both are checked.
  const themes = () => [
    { ...selectorVars(":root"), ...selectorVars(".docs-theme") },
    { ...selectorVars(":root"), ...selectorVars('html[data-theme="light"] .docs-theme') },
  ];

  it("keeps current and hover sidebar text at WCAG AA contrast", () => {
    for (const theme of themes()) {
      const accent = theme["docs-accent"];
      const background = theme["paper"];
      expect(contrastRatio(accent, background)).toBeGreaterThanOrEqual(4.5);
    }
    expect(CSS).toMatch(/\.docs-sidebar-link:hover,\s*\.docs-sidebar-link-current\s*{[^}]*color:\s*var\(--docs-accent\)/s);
  });

  it("keeps secondary button text at WCAG AA contrast", () => {
    for (const theme of themes()) {
      const text = theme["docs-button-text"];
      const background = theme["docs-button-bg"];
      expect(contrastRatio(text, background)).toBeGreaterThanOrEqual(4.5);
    }
    expect(selectorBlock(".docs-theme .portal-button-secondary")).toContain(
      "color: var(--docs-button-text)",
    );
  });
});
