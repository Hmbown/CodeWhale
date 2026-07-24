import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import { FACTS } from "./facts.generated";
import { SNIPPETS } from "./install-binary-snippets";

const root = new URL("../../", import.meta.url);

type PublicSurfaceMatrix = {
  schemaVersion: number;
  product: {
    name: string;
    description: string;
    license: string;
    terminology: Record<string, string>;
  };
  sourceCandidate: {
    version: string;
    providerCount: number;
    toolCount: number;
    sandboxBackends: string[];
  };
  latestPublishedRelease: {
    tag: string;
    version: string;
    publishedAt: string;
    url: string;
  };
  install: {
    recommended: string;
    binaries: string[];
    channels: Record<string, string>;
    androidTermux: {
      status: string;
      npm: string;
      requiresMatchingPublishedAssets: boolean;
      sourceBuild: boolean;
    };
  };
  control: {
    modes: string[];
    permissionPostures: string[];
    shortcuts: {
      mode: { chord: string; when: string };
      permissionPosture: { chord: string; when: string };
    };
  };
  toolSurface: {
    defaultActive: string[];
    actions: Record<string, string[]>;
    deferred: Record<string, string[]>;
    compatibility: {
      legacyAliases: string;
      modelVisible: boolean;
      toolSearchDiscoverable: boolean;
    };
    agentConcurrency: {
      defaultConfigured: number;
      maximumConfigured: number;
      maximumAdmitted: number;
    };
  };
  surfaces: { availableInSourceCandidate: string[] };
  trust: Record<string, string>;
  repository: {
    canonical: string;
    mirrors: string[];
    creditSources: string[];
    requiredCandidateCredits: string[];
  };
  screenshot: {
    readme: string;
    website: string;
    sourceVersion: string;
    sourceCommit: string;
    terminal: string;
    sources: string[];
  };
  [key: string]: unknown;
};

const matrix = JSON.parse(text("docs/public-surface-facts.json")) as PublicSurfaceMatrix;

function text(path: string): string {
  return readFileSync(new URL(path, root), "utf8");
}

function bytes(path: string): Buffer {
  return readFileSync(new URL(path, root));
}

function pngDimensions(image: Buffer): [number, number] {
  expect(image.subarray(1, 4).toString("ascii")).toBe("PNG");
  return [image.readUInt32BE(16), image.readUInt32BE(20)];
}

describe("public surface contracts", () => {
  it("keeps source-candidate and published-release facts distinct and aligned", () => {
    expect(matrix.schemaVersion).toBe(2);
    expect(matrix.sourceCandidate.version).toBe(FACTS.version);
    expect(matrix.sourceCandidate.providerCount).toBe(FACTS.providers.length);
    expect(matrix.sourceCandidate.toolCount).toBe(FACTS.toolCount);
    expect(matrix.sourceCandidate.sandboxBackends).toEqual(FACTS.sandboxBackends);
    expect({
      tag: matrix.latestPublishedRelease.tag,
      version: matrix.latestPublishedRelease.version,
      publishedAt: matrix.latestPublishedRelease.publishedAt,
      url: matrix.latestPublishedRelease.url,
    }).toEqual(FACTS.latestPublishedRelease);
    expect(matrix.latestPublishedRelease.version).not.toBe(matrix.sourceCandidate.version);
    expect(matrix.latestPublishedRelease).not.toHaveProperty("providerCount");
    expect(matrix.latestPublishedRelease).not.toHaveProperty("toolCount");
    expect(matrix.surfaces).not.toHaveProperty("stable");
    expect(matrix.surfaces.availableInSourceCandidate).toContain("Web client");
    expect(matrix.screenshot.sourceVersion).toBe(FACTS.version);
    expect(matrix.screenshot.sourceCommit).toMatch(/^[0-9a-f]{40}$/);
  });

  it("backs product and install claims with package and documentation content", () => {
    const readme = text("README.md");
    const npmReadme = text("npm/codewhale/README.md");
    const install = text("docs/INSTALL.md");
    const changelog = text("CHANGELOG.md");
    const license = text("LICENSE");
    const npmArtifacts = text("npm/codewhale/scripts/artifacts.js");
    const npmPackage = JSON.parse(text("npm/codewhale/package.json")) as {
      description: string;
      bin: Record<string, string>;
    };

    expect(matrix.product.name).toBe("Codewhale");
    expect(matrix.product.license).toBe("MIT");
    expect(matrix.product.description).toBe(npmPackage.description);
    // The README intro was simplified; assert the hosted-and-local claim rather
    // than the verbatim product tagline (kept in page-meta.ts / public-surface-facts.json).
    expect(readme).toMatch(/hosted or local/);
    expect(license).toContain("MIT License");
    expect(matrix.install.recommended).toBe("npm install -g codewhale");
    expect(readme).toContain(matrix.install.recommended);
    expect(Object.keys(npmPackage.bin)).toEqual(matrix.install.binaries);
    expect(matrix.install.channels).toEqual({
      npm: "published releases only",
      cargo: "published crates only",
      prebuiltArchives: "published GitHub Releases only",
      cnbMirror: "documented targets only",
    });
    expect(matrix.install.androidTermux).toEqual({
      status: "preview",
      npm: "preview",
      requiresMatchingPublishedAssets: true,
      sourceBuild: true,
    });
    expect(install).toContain("v0.9.1 source candidate");
    expect(install).toContain("unpublished source candidate");
    expect(install).toMatch(/Android \/ Termux \| arm64 \(aarch64\) \| ⚠️⁴ preview/);
    expect(install).not.toContain("wrapper is published at\nv0.9.1");
    expect(npmReadme).toMatch(/^- Android arm64 \/ Termux \(preview;/m);
    expect(npmReadme).toContain("requires matching Android assets");
    expect(npmArtifacts).toContain("android: {");
    for (const binary of [
      "codewhale-android-arm64",
      "codew-android-arm64",
      "codewhale-tui-android-arm64",
    ]) {
      expect(npmArtifacts).toContain(binary);
    }
    expect(changelog).toMatch(
      /^## \[0\.9\.1\] - (?:Unreleased candidate|\d{4}-\d{2}-\d{2})$/m,
    );
    expect(changelog).toContain("v0.9.1 source candidate");
    expect(changelog).not.toContain("compare/v0.9.1...HEAD");
  });

  it("distinguishes two Cargo packages from the three installed commands", () => {
    const installDoc = text("docs/INSTALL.md");
    const installPage = text("web/app/[locale]/install/page.tsx");

    expect(installDoc).toContain("Two Cargo packages are required");
    expect(installDoc).toContain(
      "`codewhale-cli` installs the `codewhale` and `codew` commands",
    );
    expect(installDoc).toContain(
      "Download all three matching `codewhale`, `codew`, and `codewhale-tui`",
    );
    expect(installPage).toContain(
      "# Install two Cargo packages; together they provide three commands",
    );
    expect(installPage).toContain("# codewhale + codew");
    expect(installPage).toContain("The two Cargo packages install three commands");
    expect(installPage).not.toContain("Install both binaries");
    expect(installDoc).not.toContain("install both binaries from the release tag");
    for (const platform of [
      "macos-arm64",
      "macos-x64",
      "linux-arm64",
      "linux-x64",
    ] as const) {
      expect(SNIPPETS[platform], platform).toContain(`codew-${platform}`);
      expect(SNIPPETS[platform], platform).toContain(
        `sudo mv codew-${platform} /usr/local/bin/codew`,
      );
    }
    for (const arch of ["x64", "arm64"] as const) {
      expect(SNIPPETS[`windows-${arch}`], arch).toContain(`codew-windows-${arch}.exe`);
      expect(SNIPPETS[`windows-${arch}`], arch).toContain(
        'Get-FileHash "$dest\\codew.exe"',
      );
    }
  });

  it("checks Unix release assets under their manifest filenames before renaming", () => {
    const scratch = mkdtempSync(join(tmpdir(), "codewhale-install-checksum-"));
    const mockBin = join(scratch, "bin");
    const curlPath = join(mockBin, "curl");
    const checksumPath = join(mockBin, "checksum");

    try {
      const mkdir = spawnSync("/bin/mkdir", ["-p", mockBin]);
      expect(mkdir.status, mkdir.stderr.toString()).toBe(0);

      writeFileSync(
        curlPath,
        `#!/bin/sh
output=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) shift; output="$1" ;;
    http*) url="$1" ;;
  esac
  shift
done
[ -n "$output" ] || output=$(basename "$url")
if [ "$output" = codewhale-artifacts-sha256.txt ]; then
  for platform in macos-arm64 macos-x64 linux-arm64 linux-x64; do
    for binary in codewhale codew codewhale-tui; do
      printf 'fixture-hash  %s-%s\\n' "$binary" "$platform"
    done
  done > "$output"
else
  printf 'fixture payload for %s\\n' "$output" > "$output"
fi
`,
      );
      writeFileSync(
        checksumPath,
        `#!/bin/sh
while [ "$#" -gt 0 ]; do shift; done
while read -r _hash filename; do
  if [ ! -f "$filename" ]; then
    echo "manifest target missing: $filename" >&2
    exit 1
  fi
done
`,
      );
      chmodSync(curlPath, 0o755);
      chmodSync(checksumPath, 0o755);
      for (const command of ["shasum", "sha256sum"]) {
        const link = spawnSync("/bin/ln", ["-s", checksumPath, join(mockBin, command)]);
        expect(link.status, link.stderr.toString()).toBe(0);
      }

      for (const platform of [
        "macos-arm64",
        "macos-x64",
        "linux-arm64",
        "linux-x64",
      ] as const) {
        const lines = SNIPPETS[platform].split("\n");
        const checksumLine = lines.findIndex((line) => line.includes(" -c -"));
        expect(checksumLine, platform).toBeGreaterThan(-1);
        const result = spawnSync(
          "/bin/bash",
          ["-o", "pipefail", "-eu", "-c", lines.slice(0, checksumLine + 1).join("\n")],
          {
            cwd: scratch,
            env: { ...process.env, PATH: `${mockBin}:${process.env.PATH ?? ""}` },
          },
        );
        expect(result.status, `${platform}: ${result.stderr.toString()}`).toBe(0);
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  });

  it("qualifies the resolved audit path and best-effort persistence", () => {
    const installPage = text("web/app/[locale]/install/page.tsx");

    expect(matrix.trust.audit).toContain("best-effort");
    expect(matrix.trust.audit).toContain("$CODEWHALE_HOME");
    expect(installPage).toContain(
      "const CONFIG_TREE = `$CODEWHALE_HOME/ (default: ~/.codewhale/)",
    );
    expect(installPage).toContain(
      "best-effort credential / approval / elevation events",
    );
    expect(installPage).toContain("尽力写入的凭证 / 审批 / 提权事件");
    expect(installPage).not.toContain(
      "audit.log        credential / approval / elevation audit trail",
    );
  });

  it("keeps modes, permission postures, and idle shortcuts exact", () => {
    const modes = text("docs/MODES.md");
    const keys = text("docs/KEYBINDINGS.md");
    const readme = text("README.md");
    const homepage = text("web/app/[locale]/page.tsx");
    const docsMap = text("web/lib/docs-map.ts");
    const matrixText = text("docs/public-surface-facts.json");

    expect(matrix.control.modes).toEqual(["Plan", "Act", "Operate"]);
    expect(matrix.control.permissionPostures).toEqual(["Ask", "Auto-Review", "Full Access"]);
    expect(matrix.control.shortcuts).toEqual({
      mode: { chord: "Tab", when: "composer idle" },
      permissionPosture: { chord: "Shift+Tab", when: "composer idle" },
    });
    for (const label of [...matrix.control.modes, ...matrix.control.permissionPostures]) {
      expect(modes).toContain(label);
      expect(homepage).toContain(label);
    }
    expect(modes).toContain("when the composer is");
    expect(keys).toContain("When the composer is idle");
    expect(keys).toContain("`Shift+Tab`");
    expect(readme).toContain("When the composer is idle");
    expect(`${readme}\n${modes}\n${homepage}\n${docsMap}`).not.toContain("approval posture");
    expect(matrixText).not.toContain('"approvalPostures"');

    for (const path of [
      "README.es-419.md",
      "README.ja-JP.md",
      "README.ko-KR.md",
      "README.pt-BR.md",
      "README.vi.md",
      "README.zh-CN.md",
    ]) {
      expect(text(path), path).toContain("Shift+Tab");
    }
  });

  it("enforces the ten-tool default-active policy and replay-only aliases", () => {
    const toolDoc = text("docs/TOOL_SURFACE.md");
    const design = text("docs/RUNTIME_SIMPLIFICATION_DESIGN.md").replace(/\s+/g, " ");
    const toolsPage = text("web/app/[locale]/docs/tools/page.tsx");
    const registry = text("crates/tui/src/tools/registry.rs");
    const limits = text("crates/tui/src/config/subagent_limits.rs");
    const roadmap = text("web/app/[locale]/roadmap/page.tsx");

    expect(matrix.toolSurface.defaultActive).toEqual([
      "Bash",
      "File",
      "Git",
      "Run",
      "agent",
      "remember",
      "tasks",
      "update_plan",
      "work_update",
      "tool_search",
    ]);
    expect(matrix.toolSurface.actions).toEqual({
      Bash: ["run", "wait", "interact", "cancel"],
      File: ["read", "list", "search_name", "search_content", "write", "edit", "patch"],
      Git: ["status", "diff", "log", "show", "blame"],
      Run: ["tests", "verifiers"],
    });
    expect(matrix.toolSurface.deferred).toEqual({ Web: ["search", "fetch", "wait"] });
    expect(matrix.toolSurface.compatibility).toEqual({
      legacyAliases: "replay-only",
      modelVisible: false,
      toolSearchDiscoverable: false,
    });
    expect(matrix.toolSurface.agentConcurrency).toEqual({
      defaultConfigured: 64,
      maximumConfigured: 128,
      maximumAdmitted: 1024,
    });
    expect(limits).toContain("DEFAULT_MAX_SUBAGENTS: usize = 64");
    expect(limits).toContain("MAX_SUBAGENTS: usize = 128");
    expect(limits).toContain("MAX_SUBAGENT_ADMISSION: usize = 1024");
    expect(roadmap).toContain("64 concurrent sessions by default, configurable to 128");
    expect(roadmap.indexOf('{ title: "Local web client"')).toBeLessThan(
      roadmap.indexOf('title: "Underway"'),
    );
    expect(roadmap).toContain("Implemented in the v0.9.1 source candidate");
    expect(toolDoc).toContain("exactly these ten names");
    for (const name of matrix.toolSurface.defaultActive) {
      expect(toolDoc, name).toContain(`\`${name}\``);
      expect(toolsPage, name).toContain(name);
    }
    expect(toolDoc).toContain("`Web` is a conditional, deferred action tool");
    expect(toolDoc).toContain("hidden from the model");
    expect(design).toContain(
      "The final active names are `Bash`, `File`, `Git`, `Run`, `agent`, `remember`, `tasks`, `update_plan`, `work_update`, and `tool_search`.",
    );
    expect(registry).toContain('FileTool::new("File")');
    expect(registry).toContain('GitTool::new("Git")');
    expect(registry).toContain('RunTool::new("Run")');
    expect(registry).toContain('BashTool::new("Bash")');
    expect(registry).toContain("hidden compat aliases for transcript replay");
    expect(toolsPage).not.toContain("read_file · list_dir");
    expect(toolsPage).not.toContain("rlm_open · rlm_eval");
    expect(toolsPage).not.toContain("docs/TOOL_LIFECYCLE.md");
  });

  it("states the hosted-provider privacy boundary without a false local-only promise", () => {
    const faq = text("web/app/[locale]/faq/page.tsx");
    const roadmap = text("web/app/[locale]/roadmap/page.tsx");
    const providers = text("docs/PROVIDERS.md");
    const runtime = text("docs/RUNTIME_API.md");

    expect(matrix.trust.hostedProviderBoundary).toContain("selected hosted provider");
    expect(matrix.trust.localInference).toContain("loopback local-model route");
    expect(matrix.trust.telemetry).toContain("no Codewhale product telemetry");
    expect(faq).toContain("The hosted");
    expect(faq).toContain("provider you select receives the prompt");
    expect(faq).toContain("keep model inference local");
    expect(faq).toContain("你选择的托管 provider 会收到");
    expect(faq).not.toContain("No telemetry, no cloud processing of your code");
    expect(faq).not.toContain("不会将你的代码上传到云端处理");
    expect(roadmap).not.toContain("what happens there stays there");
    expect(roadmap).not.toContain("你的数据不会离开");
    expect(providers).toMatch(/Hosted\s+routes/);
    expect(runtime).toContain("No hosted relay");
  });

  it("backs product vocabulary, contributor credit, and the exact MIT footer", () => {
    const fleet = text("docs/FLEET.md");
    const changelog = text("CHANGELOG.md");
    const contributors = text("docs/CONTRIBUTORS.md");
    const releaseCredits = text("web/lib/release-credits.ts");
    const footer = text("web/components/footer.tsx");

    expect(matrix.product.terminology).toEqual({
      Fleet: "who does the work",
      Workflow: "what order the work follows",
      Lane: "one running Workflow instance",
      Runtime: "where and how a Lane executes",
    });
    for (const [term, definition] of Object.entries(matrix.product.terminology)) {
      expect(fleet).toContain(`**${term}** = ${definition}`);
    }
    expect(matrix.repository.requiredCandidateCredits).toEqual(["@fleitz"]);
    expect(matrix.repository.mirrors.some((mirror) => mirror.includes("gitee"))).toBe(false);
    expect(changelog).toContain("PR #4673 by @fleitz");
    expect(changelog).toContain("#4674");
    expect(contributors).toContain("github.com/fleitz");
    expect(releaseCredits).toContain('"@fleitz"');
    expect(footer).toContain('{ label: "MIT license", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" }');
    expect(footer).toContain('{ label: "MIT 许可证", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" }');
    expect(footer).toContain('href="https://github.com/Hmbown/CodeWhale/releases"');
    expect(footer).toContain("GITEE_ENABLED &&");
  });

  it("keeps the README and website on one optimized canonical product screenshot", () => {
    const readmeImage = bytes(matrix.screenshot.readme);
    const websiteImage = bytes(matrix.screenshot.website);
    const digest = (image: Buffer) => createHash("sha256").update(image).digest("hex");

    expect(digest(readmeImage)).toBe(digest(websiteImage));
    expect(pngDimensions(readmeImage)).toEqual([1280, 720]);
    expect(statSync(new URL(matrix.screenshot.readme, root)).size).toBeLessThan(500_000);
    expect(matrix.screenshot.terminal).toBe("120x32");

    const readme = text("README.md");
    const homepage = text("web/app/[locale]/page.tsx");
    expect(readme).toContain("assets/screenshot.png");
    expect(homepage).toContain('src="/codewhale-tui.png"');
    expect(homepage).toContain("with no empty Work bar");
  });

  it("keeps reduced motion static without hiding the reasoning trace", () => {
    const css = text("web/app/globals.css");
    const terminalPlayer = text("web/components/terminal-player.tsx");

    expect(css).toMatch(
      /@media \(prefers-reduced-motion: reduce\)\s*\{[\s\S]*?\.tp-caret\s*\{\s*animation:\s*none;\s*\}[\s\S]*?\.ticker-track\s*\{\s*animation:\s*none;\s*\}[\s\S]*?\}/,
    );
    expect(terminalPlayer).toContain(
      'window.matchMedia("(prefers-reduced-motion: reduce)").matches',
    );
    expect(terminalPlayer).toContain("setShown(Number.MAX_SAFE_INTEGER)");
    expect(terminalPlayer).toContain("Server render shows the full trace");
  });

  it("keeps every fact-matrix source resolvable in the repository", () => {
    const sources = new Set<string>();
    const visit = (value: unknown) => {
      if (Array.isArray(value)) {
        value.forEach(visit);
      } else if (value && typeof value === "object") {
        for (const [key, child] of Object.entries(value)) {
          if (key === "sources" || key === "creditSources") {
            (child as string[]).forEach((source) => sources.add(source));
          } else {
            visit(child);
          }
        }
      }
    };
    visit(matrix);

    for (const source of sources) {
      expect(existsSync(new URL(source, root)), source).toBe(true);
    }
  });
});
