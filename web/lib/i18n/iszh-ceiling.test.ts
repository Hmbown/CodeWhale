import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = process.cwd();
const MIGRATION_HOME = join("lib", "i18n");
// One-way ceiling (#5519): the isZh migration converges or holds, never
// regresses. To lower the number: migrate branches into lib/i18n, then set
// CEILING to the new count reported by this test.
const CEILING = 28;

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === ".next" || entry === ".git") continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.[jt]sx?$/.test(entry)) out.push(full);
  }
  return out;
}

describe("isZh one-way ceiling (#5519)", () => {
  it(`keeps isZh branching outside ${MIGRATION_HOME} at <= ${CEILING} files`, () => {
    const offenders = walk(ROOT)
      .map((file) => relative(ROOT, file))
      .filter(
        (file) =>
          !file.startsWith(MIGRATION_HOME) &&
          !/\.test\.[jt]sx?$/.test(file) &&
          readFileSync(join(ROOT, file), "utf8").includes("isZh"),
      )
      .sort();
    expect(
      offenders.length,
      `isZh branching outside ${MIGRATION_HOME} (migrate, then lower the CEILING):\n${offenders.join("\n")}`,
    ).toBeLessThanOrEqual(CEILING);
  });
});
