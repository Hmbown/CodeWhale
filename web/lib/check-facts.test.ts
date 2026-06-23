/**
 * Tests for check-facts.mjs via its re-importable logic.
 *
 * We test the diffFacts helper (ported from check-facts.mjs) against
 * fixture-like objects to prove the stale-version detection works.
 * End-to-end `node scripts/check-facts.mjs` exit-code tests are not run
 * from vitest since they depend on the actual workspace file tree.
 */
import { describe, it, expect } from "vitest";

// --- inline diffFacts (same logic as check-facts.mjs) ----------------

interface ProviderFact {
  id: string;
  label: string;
  env: string;
}

interface RepoFacts {
  [key: string]: unknown;
  generatedAt: string;
  version: string | null;
  crates: string[];
  sandboxBackends: string[];
  providers: ProviderFact[];
  defaultModel: string | null;
  nodeEngines: string | null;
  toolCount: number | null;
  license: string | null;
  latestRelease: string | null;
}

function diffFacts(
  committed: Record<string, unknown>,
  fresh: Record<string, unknown>,
): Array<{ field: string; committed: unknown; fresh: unknown }> {
  const checkFields = [
    "version",
    "crates",
    "sandboxBackends",
    "providers",
    "defaultModel",
    "nodeEngines",
    "toolCount",
    "license",
  ];
  const diffs: Array<{ field: string; committed: unknown; fresh: unknown }> = [];
  for (const field of checkFields) {
    const a = JSON.stringify(committed[field] ?? null);
    const b = JSON.stringify(fresh[field] ?? null);
    if (a !== b) {
      diffs.push({ field, committed: committed[field], fresh: fresh[field] });
    }
  }
  return diffs;
}

// --- helpers ---------------------------------------------------------

function freshFacts(overrides: Partial<RepoFacts> = {}): RepoFacts {
  return {
    generatedAt: new Date().toISOString(),
    version: "0.8.64",
    crates: ["cli", "config", "tui"],
    sandboxBackends: ["landlock (Linux)", "seatbelt (macOS)"],
    providers: [
      { id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" },
      { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
    ],
    defaultModel: "deepseek-v4-pro",
    nodeEngines: ">=18",
    toolCount: 78,
    license: "MIT",
    latestRelease: null,
    ...overrides,
  };
}

// --- tests -----------------------------------------------------------

describe("diffFacts (check-facts parity)", () => {
  it("returns empty array when facts match", () => {
    const committed = freshFacts();
    const fresh = freshFacts();
    expect(diffFacts(committed, fresh)).toEqual([]);
  });

  it("detects stale version (0.8.62 vs 0.8.64)", () => {
    const committed = freshFacts({ version: "0.8.62" });
    const fresh = freshFacts({ version: "0.8.64" });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(1);
    expect(diffs[0]).toEqual({
      field: "version",
      committed: "0.8.62",
      fresh: "0.8.64",
    });
  });

  it("detects provider list drift (added Anthropic)", () => {
    const committed = freshFacts({
      providers: [{ id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" }],
    });
    const fresh = freshFacts({
      providers: [
        { id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" },
        { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
      ],
    });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(1);
    expect(diffs[0].field).toBe("providers");
  });

  it("detects stale default model", () => {
    const committed = freshFacts({ defaultModel: "deepseek-v3" });
    const fresh = freshFacts({ defaultModel: "deepseek-v4-pro" });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(1);
    expect(diffs[0].field).toBe("defaultModel");
  });

  it("detects tool count drift", () => {
    const committed = freshFacts({ toolCount: 70 });
    const fresh = freshFacts({ toolCount: 78 });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(1);
    expect(diffs[0].field).toBe("toolCount");
  });

  it("detects multiple field drifts at once", () => {
    const committed = freshFacts({
      version: "0.8.62",
      toolCount: 70,
    });
    const fresh = freshFacts({
      version: "0.8.64",
      toolCount: 78,
    });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(2);
    expect(diffs.map((d) => d.field).sort()).toEqual(["toolCount", "version"]);
  });

  it("ignores generatedAt and latestRelease changes", () => {
    const committed = freshFacts({ generatedAt: "old" });
    const fresh = freshFacts({ generatedAt: "new", latestRelease: "v0.8.64" });
    // Both generatedAt and latestRelease are excluded from checkFields.
    // Version and others match => no diffs.
    expect(diffFacts(committed, fresh)).toEqual([]);
  });

  it("handles null-to-value drift for license", () => {
    const committed = freshFacts({ license: null });
    const fresh = freshFacts({ license: "MIT" });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(1);
    expect(diffs[0].field).toBe("license");
  });

  it("handles empty arrays in committed vs populated arrays in fresh", () => {
    const committed = freshFacts({ crates: [], providers: [] });
    const fresh = freshFacts({
      crates: ["cli"],
      providers: [{ id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" }],
    });
    const diffs = diffFacts(committed, fresh);
    expect(diffs).toHaveLength(2);
    expect(diffs.map((d) => d.field).sort()).toEqual(["crates", "providers"]);
  });
});
