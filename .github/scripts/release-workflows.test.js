#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const repoRoot = path.resolve(__dirname, "..", "..");
const {
  allAssetNames,
  allReleaseAssetNames,
  BUNDLE_ASSET_NAMES,
  LEGACY_TUI_BRIDGE_ASSET_NAMES,
} = require(path.join(repoRoot, "npm", "codewhale", "scripts", "artifacts"));

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function valuesForKey(source, key) {
  const expression = new RegExp(`^\\s+${key}:\\s+([^#\\s]+)\\s*$`, "gm");
  return [...source.matchAll(expression)].map((match) => match[1]);
}

function namedStep(source, name) {
  const marker = `      - name: ${name}\n`;
  const start = source.indexOf(marker);
  assert.notEqual(start, -1, `missing workflow step: ${name}`);
  const next = source.indexOf("\n      - ", start + marker.length);
  return source.slice(start, next === -1 ? source.length : next);
}

const ci = read(".github/workflows/ci.yml");
const nightly = read(".github/workflows/nightly.yml");
const candidate = read(".github/workflows/release-candidate.yml");
const artifacts = read(".github/workflows/release-artifacts.yml");
const release = read(".github/workflows/release.yml");
const releaseDockerfile = read("packaging/docker/Dockerfile.release");
const cnb = read(".cnb.yml");
const bundles = read("scripts/release/create-release-bundles.sh");
const archiveInstaller = read("scripts/release/install.sh");
const cliDispatcher = read("crates/cli/src/lib.rs");
const runbook = read("docs/RELEASE_RUNBOOK.md");

assert.match(ci, /^  workflow_dispatch:\n    inputs:\n      expected_sha:/m);
const manualForceBlock = ci.match(
  /if \[\[ "\$\{EVENT_NAME\}" == "workflow_dispatch" \]\]; then([\s\S]*?)\n\s+if \[\[ "\$\{EVENT_NAME\}" == "schedule" \]\]; then/,
);
assert.ok(manualForceBlock, "CI must have a dedicated manual-dispatch force-full branch");
for (const output of ["heavy", "workflow", "mobile", "actions"]) {
  assert.match(manualForceBlock[1], new RegExp(`echo "${output}=true"`));
}
assert.match(manualForceBlock[1], /#EXPECTED_SHA.*-ne 40/s);
assert.match(manualForceBlock[1], /actual.*EXPECTED_SHA/s);
assert.match(
  ci,
  /run: cargo test -p codewhale-tui --test pty qa_pty::skills_opens_manager_owned_then_compatible -- --ignored --exact/,
  "CI must run the isolated Skills Manager acceptance from the consolidated PTY target",
);
assert.doesNotMatch(ci, /--test qa_pty\b/, "CI must not name the removed qa_pty target");

const expectedNightlyTargets = [
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-musl",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
].sort();
assert.deepEqual([...new Set(valuesForKey(nightly, "target"))].sort(), expectedNightlyTargets);
assert.deepEqual(
  [
    ...valuesForKey(nightly, "primary_artifact"),
    ...valuesForKey(nightly, "alias_artifact"),
  ].sort(),
  [
    "codewhale-linux-x64",
    "codew-linux-x64",
    "codewhale-linux-arm64",
    "codew-linux-arm64",
    "codewhale-macos-x64",
    "codew-macos-x64",
    "codewhale-macos-arm64",
    "codew-macos-arm64",
    "codewhale-windows-x64.exe",
    "codew-windows-x64.exe",
    "codewhale-windows-arm64.exe",
    "codew-windows-arm64.exe",
  ].sort(),
);
assert.match(
  nightly,
  /cargo build --release --locked --target \$\{\{ matrix\.target \}\} -p codewhale-cli/,
);
assert.match(nightly, /startsWith\(matrix\.target, 'x86_64-'\).*runner\.arch == 'X64'/s);
assert.match(nightly, /startsWith\(matrix\.target, 'aarch64-'\).*runner\.arch == 'ARM64'/s);
const nightlyArmMuslSetup = namedStep(nightly, "Install Linux ARM64 musl toolchain");
assert.match(nightlyArmMuslSetup, /matrix\.target == 'aarch64-unknown-linux-musl'/);
assert.match(nightlyArmMuslSetup, /apt-get install -y binutils musl-tools/);
assert.match(nightlyArmMuslSetup, /rustup target add --toolchain stable aarch64-unknown-linux-musl/);
const nightlyArmStaticSmoke = namedStep(
  nightly,
  "Verify static Linux ARM64 binary and launch",
);
assert.match(
  nightlyArmStaticSmoke,
  /matrix\.target == 'aarch64-unknown-linux-musl' && runner\.arch == 'ARM64'/,
);
assert.match(nightlyArmStaticSmoke, /readelf -l "\$\{bin_path\}"/);
assert.match(nightlyArmStaticSmoke, /grep -Fq 'INTERP'/);
assert.match(nightlyArmStaticSmoke, /"\$\{bin_path\}" --version/);
assert.doesNotMatch(nightly, /codewhale-tui/);
assert.doesNotMatch(nightly, /target\/[^\n]*\/codew(?:\.exe)?/);
assert.match(nightly, /cp "\$\{bin_path\}" "\$\{dir\}\/\$\{artifact\}"/);
assert.match(nightly, /cmp -s[\s\S]*nightly-primary[\s\S]*nightly-alias/);
assert.equal((nightly.match(/retention-days: 14/g) || []).length, 2);

assert.match(candidate, /^  workflow_dispatch:\n    inputs:\n      expected_sha:/m);
assert.doesNotMatch(candidate, /^  (push|pull_request|schedule):/m);
assert.match(candidate, /uses: \.\/\.github\/workflows\/release-artifacts\.yml/);
assert.match(candidate, /source_sha: \$\{\{ needs\.resolve\.outputs\.sha \}\}/);
assert.match(candidate, /^  web:\n/m);
assert.match(candidate, /ref: \$\{\{ needs\.resolve\.outputs\.sha \}\}/);
assert.match(candidate, /working-directory: web/);
for (const command of [
  "npm ci",
  "npm run check:facts",
  "npm run prebuild",
  "npm run check:docs",
  "npm test",
  "npm run lint",
  "npx tsc --noEmit",
  "npm run build",
]) {
  assert.match(candidate, new RegExp(`run: ${command.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
}
assert.match(candidate, /^    needs: \[resolve, web\]$/m);
assert.match(candidate, /needs\.web\.result == 'success'/);

for (const [label, workflow] of [
  ["release candidate", candidate],
  ["shared artifact", artifacts],
]) {
  for (const forbidden of [
    /contents:\s*write/,
    /packages:\s*write/,
    /softprops\/action-gh-release/,
    /docker\/login-action/,
    /docker\/build-push-action/,
    /\bgh release\b/,
    /\bnpm publish\b/,
    /\bcargo publish\b/,
    /\bgit push\b/,
  ]) {
    assert.doesNotMatch(workflow, forbidden, `${label} workflow contains publication capability`);
  }
}

for (const [label, workflow] of [
  ["release candidate", candidate],
  ["shared artifact", artifacts],
  ["public release", release],
]) {
  const remoteActions = [...workflow.matchAll(/^\s+(?:-\s+)?uses:\s+([^@\s]+)@([^#\s]+)/gm)]
    .map((match) => ({ action: match[1], ref: match[2] }))
    .filter(({ action }) => !action.startsWith("./"));
  assert.ok(remoteActions.length > 0, `${label} workflow must exercise pinned actions`);
  for (const { action, ref } of remoteActions) {
    assert.match(
      ref,
      /^[0-9a-f]{40}$/,
      `${label} action ${action} must use an audited full commit SHA`,
    );
  }
}

assert.match(artifacts, /^  workflow_call:/m);
assert.match(artifacts, /^permissions:\n  contents: read$/m);
const expectedTargets = [
  "x86_64-unknown-linux-musl",
  "aarch64-unknown-linux-musl",
  "aarch64-linux-android",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
].sort();
assert.deepEqual([...new Set(valuesForKey(artifacts, "target"))].sort(), expectedTargets);

const releaseMuslBuild = namedStep(artifacts, "Build static Linux binaries (musl)");
assert.match(releaseMuslBuild, /endsWith\(matrix\.target, '-unknown-linux-musl'\)/);
assert.match(releaseMuslBuild, /apt-get install -y binutils musl-tools/);
assert.match(releaseMuslBuild, /rustup target add --toolchain stable \$\{\{ matrix\.target \}\}/);
assert.match(
  releaseMuslBuild,
  /cargo build --profile dist --locked --target \$\{\{ matrix\.target \}\} -p codewhale-cli/,
);
const releaseStaticSmoke = namedStep(
  artifacts,
  "Verify static Linux binaries and launch on matching native runners",
);
assert.match(releaseStaticSmoke, /endsWith\(matrix\.target, '-unknown-linux-musl'\)/);
assert.match(
  releaseStaticSmoke,
  /startsWith\(matrix\.target, 'aarch64-'\) && runner\.arch == 'ARM64'/,
);
assert.match(releaseStaticSmoke, /readelf -l "\$\{bin_path\}"/);
assert.match(releaseStaticSmoke, /grep -Fq 'INTERP'/);
assert.match(releaseStaticSmoke, /"\$\{bin_path\}" --version/);

const builtAssetNames = [
  ...valuesForKey(artifacts, "cli_artifact"),
  ...valuesForKey(artifacts, "shim_artifact"),
  ...valuesForKey(artifacts, "tui_artifact"),
];
assert.equal(builtAssetNames.length, 21);
assert.deepEqual(
  [...new Set(builtAssetNames)].sort(),
  [
    ...allAssetNames().filter((name) => name !== "codewhale.bat"),
    ...LEGACY_TUI_BRIDGE_ASSET_NAMES,
  ].sort(),
);
assert.match(
  artifacts,
  /stage_binary "\$\{\{ matrix\.cli_binary \}\}" "\$\{\{ matrix\.tui_artifact \}\}"/,
  "legacy TUI bridge assets must be staged from the one compiled codewhale binary",
);
const bundleInvocations = [...bundles.matchAll(
  /^bundle (\S+) \\\n\s+\S+ \S+ (tar\.gz|zip) (""|portable)$/gm,
)].map((match) => {
  const variant = match[3] === "portable" ? "-portable" : "";
  return `codewhale-${match[1]}${variant}.${match[2]}`;
});
assert.deepEqual(bundleInvocations.sort(), [...BUNDLE_ASSET_NAMES].sort());
assert.match(artifacts, /aarch64-pc-windows-msvc/);
assert.match(artifacts, /aarch64-linux-android/);
assert.match(artifacts, /codew-windows-arm64\.exe/);
assert.match(artifacts, /CodeWhaleSetup\.exe/);
assert.match(artifacts, /assemble-release-assets\.js --verify release-assets/);
assert.match(artifacts, /CODEWHALE_SMOKE_ASSETS_DIR/);
const bundleStep = namedStep(artifacts, "Create and checksum platform archives");
assert.match(bundleStep, /git show -s --format=%ct "\$\{\{ inputs\.source_sha \}\}"/);
assert.match(
  bundleStep,
  /SOURCE_DATE_EPOCH="\$\{source_date_epoch\}"[\s\\]+bash scripts\/release\/create-release-bundles\.sh artifacts bundles/,
);
assert.doesNotMatch(bundleStep, /\bdate\b/, "bundle timestamps must come from the pinned source commit, not wall-clock time");

assert.equal(allReleaseAssetNames().length, 34);
assert.match(release, /^  artifacts:\n/m);
assert.match(release, /uses: \.\/\.github\/workflows\/release-artifacts\.yml/);
assert.doesNotMatch(release, /^  (build|bundle|windows-installer):/m);
assert.match(release, /name: codewhale-release-assets\n\s+path: artifacts/);
assert.match(release, /files: artifacts\/\*/);
assert.equal(
  (release.match(/ensure-release-assets-absent\.js/g) || []).length,
  2,
  "public release must refuse existing assets before work and immediately before upload",
);
assert.match(release, /overwrite_files:\s*false/);
assert.match(release, /fail_on_unmatched_files:\s*true/);

assert.match(release, /^  docker-build:\n/m);
assert.match(release, /^  docker:\n/m);
assert.match(release, /runner: ubuntu-latest\n\s+platform: linux\/amd64/);
assert.match(release, /runner: ubuntu-24\.04-arm\n\s+platform: linux\/arm64/);
assert.match(release, /cli_artifact: codewhale-linux-x64/);
assert.match(release, /cli_artifact: codewhale-linux-arm64/);
assert.match(release, /shim_artifact: codew-linux-x64/);
assert.match(release, /shim_artifact: codew-linux-arm64/);
assert.doesNotMatch(
  release,
  /docker\/setup-qemu-action/,
  "public container publication must not funnel both architectures through QEMU",
);
const releaseDockerBytes = namedStep(release, "Verify native release bytes");
assert.match(releaseDockerBytes, /CLI_ARTIFACT: \$\{\{ matrix\.cli_artifact \}\}/);
assert.match(releaseDockerBytes, /SHIM_ARTIFACT: \$\{\{ matrix\.shim_artifact \}\}/);
assert.match(
  releaseDockerBytes,
  /mv -- "docker-context\/bin\/\$\{CLI_ARTIFACT\}" docker-context\/bin\/codewhale/,
);
assert.match(
  releaseDockerBytes,
  /mv -- "docker-context\/bin\/\$\{SHIM_ARTIFACT\}" docker-context\/bin\/codew/,
);
assert.match(releaseDockerBytes, /cmp docker-context\/bin\/codewhale docker-context\/bin\/codew/);
const releaseDockerBuild = namedStep(release, "Assemble and push native image by digest");
assert.match(releaseDockerBuild, /context: docker-context/);
assert.match(releaseDockerBuild, /file: infra\/packaging\/docker\/Dockerfile\.release/);
assert.match(releaseDockerBuild, /platforms: \$\{\{ matrix\.platform \}\}/);
assert.match(releaseDockerBuild, /provenance: mode=max/);
assert.match(releaseDockerBuild, /sbom: true/);
assert.match(releaseDockerBuild, /push-by-digest=true/);
const releaseDockerManifest = namedStep(release, "Publish multi-architecture manifest");
assert.match(releaseDockerManifest, /Expected exactly two native image digests/);
assert.match(releaseDockerManifest, /docker buildx imagetools create/);
const releaseDockerSmoke = namedStep(release, "Verify and smoke published container");
assert.match(releaseDockerSmoke, /linux\/amd64/);
assert.match(releaseDockerSmoke, /linux\/arm64/);
assert.match(releaseDockerSmoke, /--entrypoint codewhale/);
assert.match(releaseDockerSmoke, /--entrypoint codew/);

const npmJob = release.match(/\n  npm:\n([\s\S]*?)\n  homebrew:\n/);
assert.ok(npmJob, "public release must retain a dedicated npm publication job");
assert.match(npmJob[1], /^    needs: \[release, resolve\]$/m);
assert.match(npmJob[1], /needs\.release\.result == 'success'/);
assert.match(npmJob[1], /^      contents: read$/m);
assert.match(npmJob[1], /^      id-token: write$/m);
assert.match(npmJob[1], /ref: \$\{\{ needs\.resolve\.outputs\.sha \}\}/);
assert.match(npmJob[1], /fetch-depth: 0/);
assert.match(npmJob[1], /node-version: 24/);
assert.match(npmJob[1], /registry-url: https:\/\/registry\.npmjs\.org/);
assert.match(npmJob[1], /package-manager-cache: false/);
assert.match(npmJob[1], /npm install --global npm@12\.0\.2/);
const npmTagGate = namedStep(release, "Revalidate release tag before npm publish");
const npmAssetGate = namedStep(release, "Revalidate public release assets");
const npmPublish = namedStep(release, "Publish npm wrapper with trusted publishing");
assert.match(npmTagGate, /verify-remote-tag\.sh/);
assert.match(npmAssetGate, /verify-release-assets\.sh/);
assert.match(npmAssetGate, /GH_TOKEN: \$\{\{ github\.token \}\}/);
assert.match(npmPublish, /working-directory: npm\/codewhale/);
assert.match(npmPublish, /npm publish --access public/);
assert.doesNotMatch(npmJob[1], /NPM_TOKEN|NODE_AUTH_TOKEN|secrets\./);
assert.ok(
  release.indexOf("Revalidate public release assets") <
    release.indexOf("Publish npm wrapper with trusted publishing"),
  "npm publication must follow the public exact-asset gate",
);

assert.match(releaseDockerfile, /^FROM debian:bookworm-slim$/m);
assert.match(releaseDockerfile, /ca-certificates/);
assert.match(releaseDockerfile, /libdbus-1-3/);
assert.match(releaseDockerfile, /COPY .*bin\/codewhale \/usr\/local\/bin\/codewhale/);
assert.match(releaseDockerfile, /COPY .*bin\/codew \/usr\/local\/bin\/codew/);
assert.match(releaseDockerfile, /^USER codewhale$/m);
assert.doesNotMatch(
  releaseDockerfile,
  /\bcargo\s+build\b|^FROM\s+rust:/m,
  "release container assembly must reuse the already-verified release binaries",
);

assert.match(runbook, /release[- ]candidate/i);
assert.match(runbook, /expected_sha/);
assert.match(runbook, /34/);
assert.match(runbook, /does not create a tag/i);
assert.match(runbook, /explicit.*approval/i);

const cnbRustGates = cnb.match(
  /\.rust_workspace_gates_stage: &rust_workspace_gates_stage([\s\S]*?)\n\.linux_rust_gates:/,
);
assert.ok(cnbRustGates, "CNB must retain the shared Rust workspace gate");
assert.match(
  cnbRustGates[1],
  /timeout: 45m[\s\S]*export CARGO_BUILD_JOBS=1[\s\S]*export CARGO_PROFILE_TEST_DEBUG=0[\s\S]*cargo check --workspace --all-targets --locked[\s\S]*cargo clippy --workspace --all-targets --all-features --locked -- -D warnings[\s\S]*RUST_MIN_STACK=16777216 cargo test --workspace --all-features --locked/,
  "CNB must serialize the memory-heavy Rust gate and preserve the workspace test stack contract",
);
assert.equal(
  (cnb.match(/^\s+- \*rust_workspace_gates_stage$/gm) || []).length,
  2,
  "both CNB Rust pipelines must reuse the constrained workspace gate",
);

const cnbPreflight = cnb.match(
  /\.linux_release_preflight: &linux_release_preflight([\s\S]*?)\nmain:/,
);
assert.ok(cnbPreflight, "CNB must retain a dedicated release preflight");
const cnbBuild = cnbPreflight[1].indexOf(
  "cargo build --jobs 2 --release --locked -p codewhale-cli",
);
const cnbAlias = cnbPreflight[1].indexOf(
  "cp target/release/codewhale target/release/codew",
);
const cnbSmoke = cnbPreflight[1].indexOf("node scripts/release/npm-wrapper-smoke.js");
assert.ok(cnbBuild >= 0, "CNB release preflight must build the consolidated runtime");
assert.ok(cnbAlias > cnbBuild, "CNB release preflight must materialize codew after the build");
assert.ok(cnbSmoke > cnbAlias, "CNB release preflight must materialize codew before smoke");

assert.doesNotMatch(
  archiveInstaller,
  /cargo install codewhale --locked/,
  "glibc recovery must name the published codewhale-cli crate",
);
assert.equal(
  (archiveInstaller.match(/cargo install codewhale-cli --locked/g) || []).length,
  2,
  "both glibc recovery branches must name codewhale-cli",
);
assert.match(
  archiveInstaller,
  /legacy_tui="\$BIN_DIR\/codewhale-tui"[\s\S]*install_binary "\$SCRIPT_DIR\/codewhale" "\$legacy_tui"/,
  "archive upgrades must refresh the retired TUI path from consolidated bytes",
);
assert.doesNotMatch(
  cliDispatcher,
  /codewhale_config::auto_model::classify/,
  "the CLI dispatcher must leave auto routing to the provider-aware runtime",
);

console.log(
  "Workflow contracts OK: 6-target/12-asset single-runtime nightly and exact-head 7-target/34-asset release candidate.",
);
