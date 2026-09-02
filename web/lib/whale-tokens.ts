import { readFileSync } from "node:fs";

/**
 * The whale palette as the site actually resolves it.
 *
 * `app/tokens.css` is generated from `crates/tui/src/palette/tokens.rs` by
 * `scripts/export-design-tokens.py`, so `globals.css` states which whale token
 * each site variable uses (`--paper: var(--whale-bg)`) instead of repeating the
 * hex. The contract tests still need the literal color to check parity and
 * contrast, so this reads the generated file and flattens the alias chains
 * (`--whale-success` -> `--whale-working-green` -> `#9bd66f`).
 *
 * Node-only (`node:fs`): imported by the contract tests, never by a component.
 */
const RAW: Record<string, string> = (() => {
  const css = readFileSync(new URL("../app/tokens.css", import.meta.url), "utf8");
  const raw: Record<string, string> = {};
  for (const match of css.matchAll(/--(whale-[\w-]+):\s*([^;]+);/g)) {
    raw[match[1]] = match[2].trim();
  }
  if (Object.keys(raw).length === 0) {
    throw new Error("app/tokens.css defines no --whale-* properties");
  }
  return raw;
})();

function flatten(name: string, seen = new Set<string>()): string {
  const value = RAW[name];
  if (value === undefined) throw new Error(`Unknown whale token: --${name}`);
  const alias = value.match(/^var\(--([\w-]+)\)$/);
  if (!alias) return value;
  if (seen.has(name)) throw new Error(`Cyclic whale token alias: --${name}`);
  return flatten(alias[1], seen.add(name));
}

/** Resolve a `var(--whale-*)` reference to its literal value; pass anything else through. */
export function resolveWhale(value: string): string {
  const match = value.match(/^var\(--(whale-[\w-]+)\)$/);
  return match ? flatten(match[1]) : value;
}
