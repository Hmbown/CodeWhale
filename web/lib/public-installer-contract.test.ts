import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

const installer = fileURLToPath(new URL("../public/install.sh", import.meta.url));
const fixtureRoots: string[] = [];

function executable(contents: string): Buffer {
  return Buffer.from(`#!/bin/sh\nprintf '%s\\n' '${contents}'\n`, "utf8");
}

function sha256(contents: Buffer): string {
  return createHash("sha256").update(contents).digest("hex");
}

function installFixture(
  withLegacyTui: boolean,
  options: {
    os?: "Darwin" | "Linux";
    arch?: "x86_64" | "aarch64";
    version?: string;
    glibc?: string;
    expectedStatus?: number;
  } = {},
) {
  const {
    os = "Darwin",
    arch = "x86_64",
    version,
    glibc = "2.39",
    expectedStatus = 0,
  } = options;
  const root = mkdtempSync(path.join(tmpdir(), "codewhale-web-installer-"));
  fixtureRoots.push(root);
  const releaseDir = path.join(root, "release");
  const installDir = path.join(root, "install");
  const fakeBin = path.join(root, "fake-bin");
  mkdirSync(releaseDir, { recursive: true });
  mkdirSync(installDir, { recursive: true });
  mkdirSync(fakeBin, { recursive: true });

  const runtime = executable("codewhale 0.9.5 (fixture)");
  const target = `${os === "Darwin" ? "macos" : "linux"}-${arch === "aarch64" ? "arm64" : "x64"}`;
  const assets = [`codewhale-${target}`, `codew-${target}`];
  for (const asset of assets) {
    writeFileSync(path.join(releaseDir, asset), runtime);
  }
  writeFileSync(
    path.join(releaseDir, "codewhale-artifacts-sha256.txt"),
    assets.map((asset) => `${sha256(runtime)}  ${asset}`).join("\n") + "\n",
  );

  const fakeUname = [
    "#!/bin/sh",
    'case "${1:-}" in',
    `  -s) printf '%s\\n' ${os} ;;`,
    `  -m) printf '%s\\n' ${arch} ;;`,
    `  *) printf '%s\\n' ${os} ;;`,
    "esac",
    "",
  ].join("\n");
  const fakeCurl = [
    "#!/bin/sh",
    'out=""',
    'url=""',
    'while [ "$#" -gt 0 ]; do',
    '  case "$1" in',
    '    -o) shift; out="$1" ;;',
    "    -*) ;;",
    '    *) url="$1" ;;',
    "  esac",
    "  shift",
    "done",
    'cp "$FAKE_RELEASE_DIR/${url##*/}" "$out"',
    "",
  ].join("\n");
  const fakeGetconf = [
    "#!/bin/sh",
    `printf '%s\\n' 'glibc ${glibc}'`,
    "",
  ].join("\n");
  for (const [name, contents] of [
    ["uname", fakeUname],
    ["curl", fakeCurl],
    ["getconf", fakeGetconf],
  ]) {
    const destination = path.join(fakeBin, name);
    writeFileSync(destination, contents);
    chmodSync(destination, 0o755);
  }

  const legacyPath = path.join(installDir, "codewhale-tui");
  if (withLegacyTui) {
    writeFileSync(legacyPath, executable("codewhale-tui 0.9.4 (legacy fixture)"));
    chmodSync(legacyPath, 0o755);
  }

  const result = spawnSync("/bin/sh", [installer], {
    encoding: "utf8",
    env: {
      ...process.env,
      CODEWHALE_INSTALL_DIR: installDir,
      CODEWHALE_RELEASE_BASE_URL: "https://fixtures.invalid/download",
      ...(version ? { CODEWHALE_VERSION: version } : {}),
      FAKE_RELEASE_DIR: releaseDir,
      HOME: path.join(root, "home"),
      PATH: `${fakeBin}:${process.env.PATH ?? "/usr/bin:/bin"}`,
    },
  });
  expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(expectedStatus);

  return { installDir, legacyPath, result, runtime };
}

afterEach(() => {
  for (const root of fixtureRoots.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

describe.skipIf(process.platform === "win32")("public installer compatibility contract", () => {
  it("refreshes a v0.9.4 legacy TUI command from verified consolidated bytes", () => {
    const { installDir, legacyPath, result, runtime } = installFixture(true);

    expect(readFileSync(path.join(installDir, "codewhale"))).toEqual(runtime);
    expect(readFileSync(path.join(installDir, "codew"))).toEqual(runtime);
    expect(readFileSync(legacyPath)).toEqual(runtime);
    expect(result.stdout).toContain("Refreshed legacy compatibility command:");
    expect(result.stdout).toContain("codewhale 0.9.5 (fixture)");
    expect(result.stdout).not.toContain("0.9.4");
  });

  it("does not create the retired TUI command for a clean v0.9.5 install", () => {
    const { installDir, legacyPath, result, runtime } = installFixture(false);

    expect(readFileSync(path.join(installDir, "codewhale"))).toEqual(runtime);
    expect(readFileSync(path.join(installDir, "codew"))).toEqual(runtime);
    expect(existsSync(legacyPath)).toBe(false);
    expect(result.stdout).not.toContain("Refreshed legacy compatibility command:");
  });

  it.each([undefined, "v0.9.6", "v0.9.11"])(
    "does not apply a glibc floor to static Linux arm64 version %s",
    (version) => {
      const { installDir } = installFixture(false, {
        os: "Linux",
        arch: "aarch64",
        version,
        glibc: "2.17",
      });

      expect(existsSync(path.join(installDir, "codewhale"))).toBe(true);
      expect(existsSync(path.join(installDir, "codew"))).toBe(true);
    },
  );

  it("keeps the truthful glibc preflight for an explicitly requested older Linux arm64 release", () => {
    const { result } = installFixture(false, {
      os: "Linux",
      arch: "aarch64",
      version: "v0.9.5",
      glibc: "2.35",
      expectedStatus: 1,
    });

    expect(result.stderr).toContain(
      "Codewhale v0.9.5 linux-arm64 assets require glibc 2.39 or newer",
    );
    expect(result.stderr).toContain(
      "Current v0.9.6+ assets are static musl builds",
    );
  });
});
