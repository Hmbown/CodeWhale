#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

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
const republish = read(".github/workflows/release-republish.yml");
const releaseDockerfile = read("packaging/docker/Dockerfile.release");
const cnb = read(".cnb.yml");
const bundles = read("scripts/release/create-release-bundles.sh");
const archiveInstaller = read("scripts/release/install.sh");
const cliDispatcher = read("crates/cli/src/lib.rs");
const runbook = read("docs/RELEASE_RUNBOOK.md");

const ciTestJob = ci.slice(ci.indexOf("\n  test:\n"));
assert.match(
  ciTestJob,
  /matrix\.os == 'ubuntu-latest'.*github\.event_name == 'pull_request'/,
  "heavy pull requests must select the real Ubuntu test lane",
);
assert.match(
  ciTestJob,
  /cargo nextest run --workspace --all-features --locked --profile ci/,
  "the Ubuntu pull-request lane must run workspace nextest",
);
assert.match(
  ciTestJob,
  /name: Linux test location \(CNB\)/,
  "the non-PR CNB fallback must be named explicitly",
);

const npmSmokeJob = ci.match(/^  npm-wrapper-smoke:\n([\s\S]*?)(?=^  \S)/m)?.[1];
assert.ok(npmSmokeJob, "CI must retain the required npm-wrapper job");
assert.match(
  namedStep(npmSmokeJob, "Build wrapper binaries"),
  /run: cargo build --release --locked -p codewhale-cli -p codewhale-tui/,
);
assert.match(
  namedStep(npmSmokeJob, "Smoke wrapper install and delegated entrypoints"),
  /run: node scripts\/release\/npm-wrapper-smoke\.js/,
);
const npmSmokeSteps = npmSmokeJob.split(/(?=^      - )/m).slice(1);
// Exercise the workflow's actual Boolean guards. A successful location echo
// must never substitute for the build/install smoke on a heavy pull request.
const npmSmokeCases = [
  // name, event, heavy, OS, trusted, cache success, execute, Linux deps, CNB
  ["own PR", "pull_request", true, "ubuntu-latest", true, true, true, true, false],
  ["fork PR", "pull_request", true, "ubuntu-latest", false, true, true, true, false],
  ["PR cache failure", "pull_request", true, "ubuntu-latest", false, false, true, true, false],
  ["light PR", "pull_request", false, "ubuntu-latest", true, true, false, false, false],
  ["manual Ubuntu", "workflow_dispatch", true, "ubuntu-latest", true, true, true, true, false],
  ["main Ubuntu", "push", true, "ubuntu-latest", true, true, false, false, true],
  ["main macOS", "push", true, "macos-latest", true, true, true, false, false],
  ["main Windows", "push", true, "windows-latest", true, true, true, false, false],
  ["light main", "push", false, "ubuntu-latest", true, true, false, false, false],
  ["schedule", "schedule", true, "ubuntu-latest", true, true, false, false, false],
];
for (const [label, event, heavy, os, trusted, cache, execute, linuxDeps, cnb] of npmSmokeCases) {
  const context = {
    needs: { changes: { outputs: { heavy: String(heavy), trusted: String(trusted) } } },
    github: { event_name: event },
    matrix: { os },
    steps: { sccache: { outcome: cache ? "success" : "failure" } },
  };
  const jobGuard = npmSmokeJob.match(/^    if: (.+)$/m)?.[1];
  assert.ok(jobGuard, "the wrapper job must retain its event guard");
  const jobEnabled = vm.runInNewContext(jobGuard, context);
  for (const step of npmSmokeSteps) {
    const name = step.match(/^      - (?:name|uses): (.+)$/m)?.[1];
    const guard = step.match(/^        if: (.+)$/m)?.[1];
    assert.ok(name && guard, "every wrapper step must have an explicit guard");
    let expected = execute;
    if (name === "Skip npm wrapper smoke for light change") expected = !heavy;
    else if (name === "Install Linux system dependencies") expected = linuxDeps;
    else if (name === "Linux smoke location") expected = cnb;
    else if (name === "Enable sccache" || name === "sccache stats") expected = execute && cache;
    assert.equal(
      Boolean(jobEnabled && vm.runInNewContext(guard, context)),
      expected,
      `${label}: ${name} must ${expected ? "execute" : "stay skipped"}`,
    );
  }
}
console.log(`Wrapper CI guards OK: ${npmSmokeCases.length} event cases, ${npmSmokeSteps.length} steps each.`);

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
assert.doesNotMatch(
  candidate,
  /ref: \$\{\{ needs\.resolve\.outputs\.sha \}\}/,
  "candidate jobs must checkout GITHUB_SHA, not interpolate the dispatch SHA into ref",
);
assert.match(candidate, /cache-dependency-path: web\/package-lock\.json/);
assert.match(candidate, /package-manager-cache: false/);
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
  ["release republish", republish],
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

const republishHomebrewJob = republish.match(/\n  homebrew:\n([\s\S]*)$/);
assert.ok(republishHomebrewJob, "republish must retain a Homebrew recovery job");
const republishHomebrewCheckout = namedStep(
  republishHomebrewJob[0],
  "Checkout release infrastructure",
);
assert.match(
  republishHomebrewCheckout,
  /ref: \$\{\{ github\.event\.repository\.default_branch \}\}/,
  "Homebrew recovery must use the repaired default-branch infrastructure",
);
assert.doesNotMatch(
  republishHomebrewCheckout,
  /needs\.resolve\.outputs\.sha/,
  "Homebrew recovery must not resurrect release-tag infrastructure",
);
assert.match(republishHomebrewJob[0], /gh release download "\$\{\{ needs\.resolve\.outputs\.tag \}\}"/);
assert.match(republishHomebrewJob[0], /MANIFEST: \/tmp\/codewhale-artifacts-sha256\.txt/);

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
  ...valuesForKey(artifacts, "compat_tui_artifact"),
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
  /stage_binary "\$\{\{ matrix\.cli_binary \}\}" "\$\{\{ matrix\.compat_tui_artifact \}\}"/,
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
assert.match(artifacts, /^  pin:\n/m);
assert.match(artifacts, /Require source_sha equals github\.sha/);
assert.doesNotMatch(
  artifacts,
  /ref: \$\{\{ inputs\.source_sha \}\}/,
  "artifact jobs must checkout GITHUB_SHA, not interpolate the caller SHA into ref",
);
assert.match(artifacts, /prefix-key: v1-\$\{\{ runner\.os \}\}-\$\{\{ runner\.arch \}\}-stable/);
assert.equal(
  (artifacts.match(/package-manager-cache: false/g) || []).length,
  2,
  "assemble and smoke must disable setup-node's implicit npm cache",
);
const bundleStep = namedStep(artifacts, "Create and checksum platform archives");
assert.match(bundleStep, /SOURCE_SHA: \$\{\{ github\.sha \}\}/);
assert.match(bundleStep, /git show -s --format=%ct "\$\{SOURCE_SHA\}"/);
assert.match(
  bundleStep,
  /SOURCE_DATE_EPOCH="\$\{source_date_epoch\}"[\s\\]+bash scripts\/release\/create-release-bundles\.sh artifacts bundles/,
);
assert.doesNotMatch(bundleStep, /inputs\.source_sha/);
assert.doesNotMatch(bundleStep, /\bdate\b/, "bundle timestamps must come from the pinned source commit, not wall-clock time");

const rustCacheBlocks = [...artifacts.matchAll(/uses: Swatinem\/rust-cache@[\s\S]*?(?=\n      - )/g)].map(
  (match) => match[0],
);
assert.ok(rustCacheBlocks.length >= 1, "shared artifact workflow must pin rust-cache");
for (const block of rustCacheBlocks) {
  assert.doesNotMatch(block, /github\.(event|ref|sha)|inputs\./);
}

const parity = release.match(/\n  parity:\n([\s\S]*?)\n  artifacts:\n/);
assert.ok(parity, "public release must retain a parity job");
assert.doesNotMatch(
  parity[1],
  /ref: \$\{\{ needs\.resolve\.outputs\.sha \}\}/,
  "parity must checkout GITHUB_SHA after resolve, not interpolate the tag SHA into ref",
);
assert.match(parity[1], /prefix-key: v1-\$\{\{ runner\.os \}\}-\$\{\{ runner\.arch \}\}-stable/);
const parityRustCache = [...parity[1].matchAll(/uses: Swatinem\/rust-cache@[\s\S]*?(?=\n      - )/g)].map(
  (match) => match[0],
);
assert.equal(parityRustCache.length, 1, "parity must pin exactly one rust-cache");
assert.doesNotMatch(parityRustCache[0], /github\.(event|ref|sha)|inputs\./);

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
assert.match(npmPublish, /GH_TOKEN: \$\{\{ github\.token \}\}/);
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
assert.match(runbook, /last[- ]useful[- ]log/i, "runbook must document the last-useful-log rule (#5496)");
assert.match(runbook, /404 logs/i, "runbook must document the 404-log cancellation rule (#5496)");

const cnbRustGates = cnb.match(
  /\.rust_workspace_gates_stage: &rust_workspace_gates_stage([\s\S]*?)\n\.linux_rust_gates:/,
);
assert.ok(cnbRustGates, "CNB must retain the shared Rust workspace gate");
assert.match(
  cnbRustGates[1],
  /timeout: 45m[\s\S]*export CARGO_BUILD_JOBS=1[\s\S]*export CARGO_PROFILE_TEST_DEBUG=0[\s\S]*cargo check --workspace --all-targets --locked[\s\S]*cargo clippy --workspace --all-targets --all-features --locked -- -D warnings[\s\S]*RUST_MIN_STACK=16777216 cargo test --workspace --all-features --locked/,
  "CNB must serialize the memory-heavy Rust gate and preserve the workspace test stack contract",
);
assert.match(
  cnbRustGates[1],
  /export HOME="\$\{hermetic_home\}"[\s\S]*export CODEWHALE_HOME="\$\{hermetic_home\}\/\.codewhale"[\s\S]*unset CODEWHALE_CONFIG_PATH DEEPSEEK_CONFIG_PATH DEEPSEEK_HOME/,
  "CNB workspace tests must not read a populated runner ~/.codewhale (#5355)",
);

const nextest = read(".config/nextest.toml");
const integrationGroup = nextest.search(/^filter = 'binary\(integration\)'$/m);
const telemetryGroup = nextest.indexOf(
  "filter = 'binary(integration) & test(/^telemetry_contract::/)'",
);
const execGroup = nextest.indexOf(
  "filter = 'binary(integration) & test(/^exec_persistent_service::/)'",
);
assert.ok(integrationGroup >= 0, "nextest must bound the integration binary");
assert.ok(
  telemetryGroup >= 0 && telemetryGroup < integrationGroup,
  "telemetry-contract override must precede binary(integration); first matching group wins",
);
assert.ok(
  execGroup >= 0 && execGroup < integrationGroup,
  "exec_persistent_service override must precede binary(integration); first matching group wins",
);
assert.match(nextest, /exec-persistent-service = \{ max-threads = 1 \}/);
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

const cnbTagRelease = cnb.match(/\$:\n  tag_push:\n([\s\S]*)$/);
assert.ok(cnbTagRelease, "CNB must retain a tag release pipeline");
const cnbTagStamp = cnbTagRelease[1].indexOf(
  'export CODEWHALE_BUILD_SHA="$commit_sha"',
);
const cnbTagBuild = cnbTagRelease[1].indexOf(
  "cargo build --jobs 2 --release --locked \\",
);
assert.match(cnbTagRelease[1], /checkout_sha="\$\(git rev-parse 'HEAD\^\{commit\}'\)"/);
assert.match(cnbTagRelease[1], /commit_sha="\$\{CNB_COMMIT:-\$\{checkout_sha\}\}"/);
assert.match(cnbTagRelease[1], /CNB_COMMIT[\s\S]*does not match checkout[\s\S]*exit 1/);
assert.ok(cnbTagStamp >= 0, "CNB tag releases must stamp the consolidated runtime");
assert.ok(cnbTagBuild > cnbTagStamp, "CNB tag releases must stamp before compiling");

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
// The archive installer never overwrites an existing command: it validates the
// retired TUI path against the consolidated bytes and leaves upgrades to
// `codewhale update`, which migrates `codewhale-tui` beside the canonical pair.
assert.match(
  archiveInstaller,
  /legacy_tui="\$BIN_DIR\/codewhale-tui"[\s\S]*check_destination "\$SCRIPT_DIR\/codewhale" "\$legacy_tui"/,
  "archive installs must validate the retired TUI path against consolidated bytes",
);
assert.doesNotMatch(
  archiveInstaller,
  /install_binary "\$SCRIPT_DIR\/codewhale" "\$legacy_tui"/,
  "archive installs must not overwrite an existing retired TUI command",
);
assert.doesNotMatch(
  cliDispatcher,
  /codewhale_config::auto_model::classify/,
  "the CLI dispatcher must leave auto routing to the provider-aware runtime",
);

// #5496: every release-lane job carries an explicit `timeout-minutes`.
//
// GitHub's default is 360 minutes, so an assigned-but-dead runner sits for six
// hours before anything reclaims it — observed on the v0.9.9 train as a job
// stuck `in_progress` with 404 logs. Timeouts are containment, not recovery:
// the runbook keeps the 404-log cancel/rerun rule for infrastructure failures.
//
// A job that calls a reusable workflow (`uses:`) cannot carry the key at all —
// GitHub rejects it — so the callee owns its own caps. That is why the artifact
// bounds live in release-artifacts.yml rather than in its callers.
function jobsWithoutTimeout(source) {
  const lines = source.split("\n");
  const jobsAt = lines.findIndex((line) => /^jobs:\s*$/.test(line));
  assert.notEqual(jobsAt, -1, "workflow must declare jobs");
  const offenders = [];
  for (let i = jobsAt + 1; i < lines.length; i += 1) {
    const header = lines[i].match(/^  ([A-Za-z0-9_-]+):\s*$/);
    if (!header) continue;
    let reusable = false;
    let capped = false;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (/^  [A-Za-z0-9_-]+:\s*$/.test(lines[j])) break;
      if (/^    uses:/.test(lines[j])) reusable = true;
      if (/^    timeout-minutes:\s*\d+\s*$/.test(lines[j])) capped = true;
    }
    if (!reusable && !capped) offenders.push(header[1]);
  }
  return offenders;
}

assert.deepEqual(
  jobsWithoutTimeout("jobs:\n  uncapped:\n    runs-on: ubuntu-latest\n"),
  ["uncapped"],
  "jobsWithoutTimeout must detect an uncapped job",
);
assert.deepEqual(
  jobsWithoutTimeout("jobs:\n  reusable:\n    uses: ./.github/workflows/reusable.yml\n"),
  [],
  "jobsWithoutTimeout must skip reusable workflow callers",
);
assert.deepEqual(
  jobsWithoutTimeout("jobs:\n  capped:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n"),
  [],
  "jobsWithoutTimeout must accept a capped job",
);

for (const [name, source] of [
  ["release-candidate.yml", candidate],
  ["release-artifacts.yml", artifacts],
  ["release.yml", release],
  ["release-republish.yml", republish],
  ["ci.yml", ci],
  ["nightly.yml", nightly],
]) {
  assert.deepEqual(
    jobsWithoutTimeout(source),
    [],
    `${name}: every job must set timeout-minutes (#5496)`,
  );
}

// The Windows artifact build historically runs 40-45 minutes, so its cap has to
// keep real margin — a tight bound here fails healthy releases.
const buildTimeout = artifacts.match(/^  build:\n(?:.*\n)*?    timeout-minutes: (\d+)$/m);
assert.ok(buildTimeout, "release-artifacts build job must be capped");
assert.ok(
  Number(buildTimeout[1]) >= 60,
  `artifact build cap ${buildTimeout[1]}m leaves no margin over a healthy 40-45m Windows build`,
);

function jobTimeout(source, job) {
  const match = source.match(
    new RegExp(`^  ${job}:\\n(?:.*\\n)*?    timeout-minutes: (\\d+)$`, "m"),
  );
  assert.ok(match, `${job} must declare timeout-minutes`);
  return Number(match[1]);
}

// Pin the measured release-lane budget: fast setup and packaging fail quickly,
// while cross-platform compilation keeps real margin over the 40-45m Windows
// build observed on the release train.
assert.equal(jobTimeout(candidate, "resolve"), 10);
assert.equal(jobTimeout(candidate, "web"), 15);
assert.equal(jobTimeout(artifacts, "pin"), 10);
assert.equal(jobTimeout(artifacts, "build"), 90);
for (const job of ["bundle", "windows-installer", "assemble", "smoke"]) {
  assert.equal(jobTimeout(artifacts, job), 15, `${job} must keep the 15m packaging cap`);
}
assert.equal(jobTimeout(nightly, "build"), 90);
assert.equal(jobTimeout(release, "resolve"), 10);
// The v0.9.12 tag push finished every parity step and was then cancelled at
// 20 minutes inside rust-cache's post-run save; 45 keeps that margin.
assert.equal(jobTimeout(release, "parity"), 45);

console.log(
  "Workflow contracts OK: 6-target/12-asset single-runtime nightly and exact-head 7-target/34-asset release candidate.",
);
