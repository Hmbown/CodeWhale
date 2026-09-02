import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { resolveWhale } from "./whale-tokens";

const CSS = readFileSync(new URL("../app/globals.css", import.meta.url), "utf8");
const TUI_TOKENS = readFileSync(
  new URL("../../crates/tui/src/palette/tokens.rs", import.meta.url),
  "utf8",
);

function rustRgb(name: string): string {
  const match = TUI_TOKENS.match(
    new RegExp(`pub const ${name}_RGB:[^=]+\\= \\((\\d+), (\\d+), (\\d+)\\)`),
  );
  if (!match) throw new Error(`Missing Rust RGB token: ${name}_RGB`);
  return `#${match
    .slice(1)
    .map((channel) => Number(channel).toString(16).padStart(2, "0"))
    .join("")}`;
}

function selectorBlock(selector: string): string {
  const match = CSS.match(
    new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`, "s"),
  );
  if (!match) throw new Error(`Missing CSS selector: ${selector}`);
  return match[1];
}

// globals.css names the whale token (`--paper: var(--whale-bg)`) rather than
// repeating its hex; resolve one hop through the generated app/tokens.css.
function cssHexIn(block: string, name: string): string {
  const match = block.match(new RegExp(`--${name}:\\s*([^;]+);`, "i"));
  if (!match) throw new Error(`Missing CSS token in block: --${name}`);
  const value = resolveWhale(match[1].trim());
  if (!/^#[0-9a-f]{6}$/i.test(value)) {
    throw new Error(`--${name} is not a hex color: ${value}`);
  }
  return value.toLowerCase();
}

const ROOT = selectorBlock(":root");
const DOCS_LIGHT = selectorBlock('html[data-theme="light"] .docs-theme');

describe("Tideline public-surface contract", () => {
  it("grounds the site field, ink, and action in the TUI dark whale tokens", () => {
    // The site default IS the deep-ocean stage: root tokens are the WHALE_*
    // dark palette, not a light sheet with a dark overlay.
    expect(cssHexIn(ROOT, "paper")).toBe(rustRgb("WHALE_BG"));
    expect(cssHexIn(ROOT, "paper-deep")).toBe(rustRgb("WHALE_PANEL"));
    expect(cssHexIn(ROOT, "paper-edge")).toBe(rustRgb("WHALE_BORDER"));
    expect(cssHexIn(ROOT, "ink")).toBe(rustRgb("WHALE_TEXT_BODY"));
    expect(cssHexIn(ROOT, "ink-soft")).toBe(rustRgb("WHALE_TEXT_SOFT"));
    expect(cssHexIn(ROOT, "ink-mute")).toBe(rustRgb("WHALE_TEXT_MUTED"));
    expect(cssHexIn(ROOT, "indigo")).toBe(rustRgb("WHALE_ACTION"));
    expect(cssHexIn(ROOT, "ocean-deep")).toBe(rustRgb("WHALE_BG"));
    expect(cssHexIn(ROOT, "action-on-dark")).toBe(rustRgb("WHALE_ACTION"));
    expect(cssHexIn(ROOT, "signal-gold")).toBe(rustRgb("WHALE_HUMAN"));
    expect(cssHexIn(ROOT, "ocean-current")).toBe(rustRgb("WHALE_ICE"));
    expect(cssHexIn(ROOT, "jade")).toBe(rustRgb("WHALE_WORKING_GREEN"));
  });

  it("keeps the opt-in docs light sheet on the TUI light tokens", () => {
    expect(cssHexIn(DOCS_LIGHT, "paper")).toBe(rustRgb("LIGHT_SURFACE"));
    expect(cssHexIn(DOCS_LIGHT, "paper-deep")).toBe(rustRgb("LIGHT_ELEVATED"));
    expect(cssHexIn(DOCS_LIGHT, "ink")).toBe(rustRgb("LIGHT_TEXT_BODY"));
    expect(cssHexIn(DOCS_LIGHT, "indigo")).toBe(rustRgb("LIGHT_ACTION"));
  });

  it("renders the whale mark in its white brand ink while controls use action blue", () => {
    expect(CSS).toMatch(/\.codewhale-mark-primary \{ fill: var\(--mark-ink\); \}/);
    expect(CSS).toMatch(/\.portal-button-primary[\s\S]*background: var\(--indigo\)/);
    expect(CSS).toMatch(/\.nav-link::after[\s\S]*background: var\(--indigo\)/);
  });

  it("keeps localized navigation controls inside compact viewports", () => {
    const mobile = CSS.split("@media (max-width: 520px)")[1];

    expect(mobile).toMatch(/\.site-nav-inner\s*\{\s*gap:\s*0\.5rem/);
    expect(mobile).toMatch(/\.site-nav-actions\s*\{[\s\S]*?min-width:\s*0/);
    expect(mobile).toMatch(
      /\.site-nav-actions select\s*\{[\s\S]*?width:\s*6\.75rem;[\s\S]*?min-width:\s*0/,
    );
    expect(CSS).toMatch(/\.paper-wordmark-mark\s*\{[^}]*height:\s*22px;/);
    expect(CSS).toMatch(/\.paper-wordmark-logo\s*\{[^}]*height:\s*20px;/);
    expect(CSS).toMatch(/\.site-nav-actions\s*\{[\s\S]*?flex-shrink:\s*0/);
    expect(CSS).toMatch(/\.site-nav-actions\s*>\s*\*\s*\{\s*flex-shrink:\s*0/);
    expect(CSS).toMatch(/@media \(max-width: 900px\)[\s\S]*?\.site-github-link\s*\{\s*display:\s*none/);
    expect(mobile).not.toMatch(/body:has\(\.product-home\) \.site-nav-actions select/);
    // The locale <select> and the home wordmark must keep a usable hit
    // target on every viewport, not only below 520px. Long native option
    // labels and 2xl companion text used to collapse the wordmark to 0.
    expect(CSS).toMatch(
      /\.site-nav-actions select\s*\{\s*width:\s*6\.75rem;\s*max-width:\s*6\.75rem;\s*min-width:\s*0;/,
    );
    // `min-width` is the floor that keeps the wordmark clickable; the shrink
    // factor stays at 1 so the compact controls are never the ones pushed
    // past `overflow-x: clip` when the row is over budget.
    expect(CSS).toMatch(/\.paper-wordmark\s*\{[^}]*flex:\s*0 1 auto;[^}]*min-width:\s*8\.75rem;/);
    expect(CSS).not.toMatch(/\.paper-wordmark\s*\{[^}]*flex:\s*0 0 auto;/);
  });
});
