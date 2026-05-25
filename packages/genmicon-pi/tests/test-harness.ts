import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export { assert };

export async function withTempDir<T>(name: string, run: (path: string) => T | Promise<T>): Promise<T> {
  const dir = mkdtempSync(join(tmpdir(), `genmicon-pi-${name}-`));
  try {
    return await run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

export function repoRoot(): string {
  return new URL("../../..", import.meta.url).pathname;
}
