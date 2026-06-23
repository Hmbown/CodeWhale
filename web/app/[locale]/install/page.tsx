import Link from "next/link";
import { Seal } from "@/components/seal";
import { InstallCodeBlock } from "@/components/install-code-block";
import { InstallBinary } from "@/components/install-binary";
import { getFacts } from "@/lib/facts";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/install",
    locale,
    title: isZh ? "安装 · CodeWhale" : "Install · CodeWhale",
    description: isZh
      ? "一行 npm install -g codewhale 安装 CodeWhale，也支持 Cargo、GitHub Releases、CNB 镜像、Homebrew、预编译二进制、Docker 和源码编译。"
      : "Install CodeWhale with npm install -g codewhale — or via cargo, GitHub Releases, the CNB mirror, Homebrew, prebuilt binaries, Docker, or from source.",
  });
}

const NPM_INSTALL = `npm install -g codewhale`;
const CARGO_INSTALL = `cargo install codewhale-cli --locked
cargo install codewhale-tui --locked`;
const FIRST_RUN = `codewhale`;

const UPDATE = `codewhale update`;

const SET_KEY_BASH = `export DEEPSEEK_API_KEY=sk-...`;
const SET_KEY_AUTH = `codewhale auth set --provider deepseek --api-key sk-...`;

const RELEASE_DOWNLOAD = `# Download your platform archive (incl. Linux riscv64):
https://github.com/Hmbown/CodeWhale/releases/latest`;
const cnbInstall = (tag: string) => `cargo install --git https://cnb.cool/codewhale.net/codewhale --tag ${tag} codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag ${tag} codewhale-tui --locked --force`;
const TUNA_CONFIG = `# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"`;
const TUNA_INSTALL = `cargo install codewhale-cli --locked
cargo install codewhale-tui --locked`;

const BREW = `brew tap Hmbown/deepseek-tui
brew install deepseek-tui`;

const DOCKER = `docker volume create codewhale-home
docker run --rm -it \\
  -e DEEPSEEK_API_KEY=$DEEPSEEK_API_KEY \\
  -v codewhale-home:/home/codewhale/.codewhale \\
  -v "$PWD:/workspace" -w /workspace \\
  ghcr.io/hmbown/codewhale:latest`;

const FROM_SOURCE = `git clone https://github.com/Hmbown/CodeWhale
cd codewhale
cargo build --release --locked

# Install both binaries from the local checkout
cargo install --path crates/cli --locked   # codewhale
cargo install --path crates/tui --locked   # codewhale-tui`;

const CONFIG_TREE = `~/.codewhale/
├── config.toml      api keys, model, hooks, profiles
├── mcp.json         MCP server definitions
├── skills/          user skills (each with SKILL.md)
├── sessions/        checkpoints + offline queue
├── tasks/           background task store
└── audit.log        credential / approval / elevation audit trail

./.codewhale/        project-scoped config (optional, per-repo)`;

const CONFIG_TREE_ZH = `~/.codewhale/
├── config.toml      API 密钥、模型、钩子、配置集
├── mcp.json         MCP 服务器定义
├── skills/          用户技能（每个含 SKILL.md）
├── sessions/        检查点 + 离线队列
├── tasks/           后台任务存储
└── audit.log        凭证 / 审批 / 提权审计日志

./.codewhale/        项目级配置（可选，每个仓库）`;

export default async function InstallPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const facts = await getFacts();
  const tag = facts.version ? `v${facts.version}` : "v0.8.x";
  const verify = `codewhale --version   # ${facts.version ?? "prints the installed version"}
codewhale doctor`;

  const copyLabel = isZh ? "复制" : "Copy";
  const copiedLabel = isZh ? "已复制 ✓" : "Copied ✓";

  return (
    <>
      {/* ① INSTALL */}
      <section className="mx-auto max-w-[1100px] px-6 pt-12 pb-10">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="装" />
          <div className="eyebrow">{isZh ? "01 · 安装" : "01 · Install"}</div>
        </div>
        <h1 className="font-display tracking-crisp mb-6">
          {isZh ? (
            <>安装 <span className="font-cjk text-indigo text-5xl ml-2">Install</span></>
          ) : (
            <>Install <span className="font-cjk text-indigo text-5xl ml-2">安装</span></>
          )}
        </h1>

        <div className="space-y-3">
          <InstallCodeBlock cmd={NPM_INSTALL} copyLabel={copyLabel} copiedLabel={copiedLabel} />
          <InstallCodeBlock cmd={FIRST_RUN} copyLabel={copyLabel} copiedLabel={copiedLabel} />
        </div>

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              npm wrapper（Node 18+）会从 GitHub Releases 下载经 SHA-256 校验的二进制，并安装{" "}
              <code className="inline">codewhale</code>、<code className="inline">codew</code> 和{" "}
              <code className="inline">codewhale-tui</code> 三个命令。
              下方「其他安装方式」列出 Cargo、GitHub Releases、CNB、国内镜像、Homebrew、预编译二进制和 Docker。
            </>
          ) : (
            <>
              The npm wrapper (Node 18+) downloads SHA-256-verified binaries from GitHub Releases
              and installs <code className="inline">codewhale</code>,{" "}
              <code className="inline">codew</code>, and{" "}
              <code className="inline">codewhale-tui</code>. See{" "}
              <a href="#other-ways" className="body-link">Other ways to install</a> below for
              cargo, GitHub Releases, CNB, Homebrew, prebuilt binaries, Docker, or mainland China
              mirrors.
            </>
          )}
        </p>
      </section>

      {/* ② VERIFY */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="验" />
          <div className="eyebrow">{isZh ? "02 · 验证" : "02 · Verify"}</div>
        </div>

        <InstallCodeBlock cmd={verify} copyLabel={copyLabel} copiedLabel={copiedLabel} />

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              <code className="inline">codewhale doctor</code> 检查 API 密钥、网络、沙箱可用性、
              MCP 服务器，并将完整报告写入{" "}
              <code className="inline">~/.codewhale/doctor.log</code>。
            </>
          ) : (
            <>
              <code className="inline">codewhale doctor</code> checks your API key, network,
              sandbox availability, and MCP servers. Full report is written to{" "}
              <code className="inline">~/.codewhale/doctor.log</code>.
            </>
          )}
        </p>
      </section>

      {/* ③ UPDATE */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="新" />
          <div className="eyebrow">{isZh ? "03 · 更新" : "03 · Update"}</div>
        </div>

        <InstallCodeBlock cmd={UPDATE} copyLabel={copyLabel} copiedLabel={copiedLabel} />

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              检查 GitHub Releases 是否有新版本并就地替换二进制。
              通过包管理器安装的话，用包管理器升级更稳：npm 安装的运行{" "}
              <code className="inline">npm update -g codewhale</code>；
              Cargo 安装的重跑两个 <code className="inline">cargo install</code> 命令并加{" "}
              <code className="inline">--force</code>；
              旧版 Homebrew tap 用 <code className="inline">brew upgrade deepseek-tui</code>。
            </>
          ) : (
            <>
              Checks GitHub Releases for a newer version and replaces the binary in place. If you
              installed via a package manager, prefer it instead: npm users run{" "}
              <code className="inline">npm update -g codewhale</code>; cargo users re-run both{" "}
              <code className="inline">cargo install</code> commands with{" "}
              <code className="inline">--force</code>; the legacy Homebrew tap updates with{" "}
              <code className="inline">brew upgrade deepseek-tui</code>.
            </>
          )}
        </p>
      </section>

      {/* ④ FIRST RUN */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="始" />
          <div className="eyebrow">{isZh ? "04 · 首次运行" : "04 · First run"}</div>
        </div>

        <ol className="space-y-6 max-w-2xl">
          <li>
            <div className="font-display text-lg mb-2">
              {isZh ? "① 获取 API 密钥" : "① Get an API key"}
            </div>
            <p className="text-sm text-ink-soft leading-relaxed">
              {isZh ? (
                <>
                  在{" "}
                  <a href="https://platform.deepseek.com" className="body-link">
                    platform.deepseek.com
                  </a>{" "}
                  注册并创建密钥，格式为 <code className="inline">sk-...</code>。
                </>
              ) : (
                <>
                  Sign up at{" "}
                  <a href="https://platform.deepseek.com" className="body-link">
                    platform.deepseek.com
                  </a>{" "}
                  and create a key (format: <code className="inline">sk-...</code>).
                </>
              )}
            </p>
          </li>

          <li>
            <div className="font-display text-lg mb-2">
              {isZh ? "② 设置密钥" : "② Set the key"}
            </div>
            <div className="space-y-2">
              <InstallCodeBlock cmd={SET_KEY_BASH} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="text-xs text-ink-mute">
                {isZh ? "或保存到 ~/.codewhale/config.toml：" : "Or persist it to ~/.codewhale/config.toml:"}
              </p>
              <InstallCodeBlock cmd={SET_KEY_AUTH} copyLabel={copyLabel} copiedLabel={copiedLabel} />
            </div>
          </li>

          <li>
            <div className="font-display text-lg mb-2">
              {isZh ? "③ 在项目目录中运行" : "③ Run it in a project"}
            </div>
            <InstallCodeBlock cmd={`cd path/to/project\ncodewhale`} copyLabel={copyLabel} copiedLabel={copiedLabel} />
            <p className="mt-3 text-sm text-ink-soft leading-relaxed">
              {isZh ? (
                <>
                  默认 Plan 模式（只读调查）。按{" "}
                  <kbd className="font-mono text-xs px-1 hairline-t hairline-b hairline-l hairline-r">Tab</kbd>{" "}
                  切换到 Agent 模式（执行工具，按需审批）。再按一次进入 YOLO 模式（自动批准）。
                </>
              ) : (
                <>
                  Plan mode (read-only) is the default. Press{" "}
                  <kbd className="font-mono text-xs px-1 hairline-t hairline-b hairline-l hairline-r">Tab</kbd>{" "}
                  to switch to Agent mode (tool execution, per-action approval). Press again for
                  YOLO (auto-approve).
                </>
              )}
            </p>
          </li>
        </ol>
      </section>

      {/* ⑤ OTHER WAYS TO INSTALL */}
      <section id="other-ways" className="bg-paper-deep hairline-t hairline-b">
        <div className="mx-auto max-w-[1100px] px-6 py-12">
          <div className="flex items-baseline gap-4 mb-5">
            <Seal char="备" />
            <div className="eyebrow">{isZh ? "05 · 其他安装方式" : "05 · Other ways to install"}</div>
          </div>
          <h2 className="font-display text-3xl mb-2">
            {isZh ? "其他安装方式" : "Other ways to install"}
          </h2>
          <p className="text-sm text-ink-soft max-w-2xl mb-10">
            {isZh
              ? "如果上面的 npm 路径不适合你，从下面找到匹配你情况的一条。每条都安装同一组 codewhale / codewhale-tui 二进制。"
              : "If the npm path above doesn't fit your setup, pick the row that matches your situation. Every path installs the same codewhale / codewhale-tui binary pair."}
          </p>

          <div className="space-y-10">
            {/* Cargo */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                {isZh ? "Rust 工具链" : "Rust toolchain"}
              </div>
              <InstallCodeBlock cmd={CARGO_INSTALL} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="mt-3 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh ? (
                  <>
                    编译并安装 <code className="inline">codewhale</code> 和 <code className="inline">codewhale-tui</code> 到 <code className="inline">~/.cargo/bin</code>。
                    需要 Rust 1.88+；如未安装可访问 <a href="https://rustup.rs" className="body-link">rustup.rs</a>。
                  </>
                ) : (
                  <>
                    Compiles and installs <code className="inline">codewhale</code> and{" "}
                    <code className="inline">codewhale-tui</code> to{" "}
                    <code className="inline">~/.cargo/bin</code>. Requires Rust 1.88+; install via{" "}
                    <a href="https://rustup.rs" className="body-link">rustup.rs</a> if you don&apos;t have it.
                  </>
                )}
              </p>
            </div>

            {/* GitHub Release */}
            <div className="rounded-lg border border-ink/12 bg-white/70 p-5">
              <div className="font-display text-lg mb-3">{isZh ? "GitHub Releases" : "GitHub Releases"}</div>
              <InstallCodeBlock cmd={RELEASE_DOWNLOAD} copyLabel={copyLabel} copiedLabel={copiedLabel} />
            </div>

            {/* CNB */}
            <div className="rounded-lg border border-ink/12 bg-white/70 p-5">
              <div className="font-display text-lg mb-3">{isZh ? "CNB 镜像" : "CNB mirror"}</div>
              <InstallCodeBlock cmd={cnbInstall(tag)} copyLabel={copyLabel} copiedLabel={copiedLabel} />
            </div>

            {/* Mainland China network */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                {isZh ? "中国大陆网络" : "Mainland China network"}
              </div>
              <p className="text-sm text-ink-soft leading-relaxed max-w-2xl mb-3">
                {isZh ? (
                  <>
                    Cargo 经清华 Tuna 镜像——添加到 <code className="inline">~/.cargo/config.toml</code>：
                  </>
                ) : (
                  <>
                    Cargo via Tsinghua Tuna mirror — add to{" "}
                    <code className="inline">~/.cargo/config.toml</code>:
                  </>
                )}
              </p>
              <InstallCodeBlock cmd={TUNA_CONFIG} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <div className="mt-3">
                <InstallCodeBlock cmd={TUNA_INSTALL} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              </div>

              <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh ? (
                  <>
                    npm 安装时设置 <code className="inline">CODEWHALE_USE_CNB_MIRROR=1</code>，
                    wrapper 会改从 CNB 镜像下载二进制而不是 GitHub。Cargo + Tuna 或 CNB
                    路径同样可以绕开 GitHub 下载瓶颈。
                    DeepSeek API（<code className="inline">api.deepseek.com</code>）在国内直连，无需代理。
                  </>
                ) : (
                  <>
                    For the npm path, set{" "}
                    <code className="inline">CODEWHALE_USE_CNB_MIRROR=1</code> and the wrapper
                    downloads binaries from the CNB mirror instead of GitHub. Cargo + Tuna or the
                    CNB path also routes around GitHub download bottlenecks. The DeepSeek API at{" "}
                    <code className="inline">api.deepseek.com</code> is reachable from mainland China
                    without a proxy.
                  </>
                )}
              </p>
            </div>

            {/* Homebrew */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                Homebrew{" "}
                <span className="text-ink-mute font-mono normal-case tracking-normal">
                  {isZh ? "· macOS / Linux · 旧版 tap" : "· macOS / Linux · legacy tap"}
                </span>
              </div>
              <InstallCodeBlock cmd={BREW} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="mt-3 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh
                  ? "这是旧版 deepseek-tui tap，在 formula 重命名为 codewhale 期间保留以保证兼容，安装的同样是当前版本的二进制。"
                  : "This is the legacy deepseek-tui tap, kept for compatibility while the formula is renamed to codewhale. It installs the same current-release binaries."}
              </p>
            </div>

            {/* Prebuilt binary */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                {isZh ? "预编译二进制" : "Prebuilt binary"}{" "}
                <span className="text-ink-mute font-mono normal-case tracking-normal">
                  {isZh ? "· 已自动检测" : "· auto-detected"}
                </span>
              </div>
              <InstallBinary
                copyLabel={copyLabel}
                copiedLabel={copiedLabel}
                verifyHeading={isZh ? "校验 SHA256" : "Verify checksum"}
              />
            </div>

            {/* Docker */}
            <div>
              <div className="eyebrow mb-2 text-indigo">Docker</div>
              <InstallCodeBlock cmd={DOCKER} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="mt-3 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh
                  ? "发布镜像位于 GHCR。需要固定版本时，把 latest 替换成具体的发布标签。"
                  : "The release image is published to GHCR. Replace latest with a release tag when you need a pinned version."}
              </p>
            </div>

            {/* From source */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                {isZh ? "从源码编译" : "From source"}
              </div>
              <InstallCodeBlock cmd={FROM_SOURCE} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="mt-3 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh
                  ? "适合本地修改 workspace 或贡献补丁。"
                  : "Useful for working on the workspace itself or contributing patches."}
              </p>
            </div>
          </div>
        </div>
      </section>

      {/* ⑥ WHERE CONFIG LIVES */}
      <section className="mx-auto max-w-[1100px] px-6 py-12">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="件" />
          <div className="eyebrow">{isZh ? "06 · 配置文件在哪" : "06 · Where config lives"}</div>
        </div>
        <InstallCodeBlock cmd={isZh ? CONFIG_TREE_ZH : CONFIG_TREE} copyLabel={copyLabel} copiedLabel={copiedLabel} />
        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              项目级 <code className="inline">./.codewhale/</code> 目录是可选的——每个仓库可有独立的 MCP 服务器、钩子、
              技能和配置覆盖（例如提供商密钥）。
              首次运行时，如果缺少配置文件，系统会询问是否交互式创建。旧版 <code className="inline">~/.deepseek</code> 和 <code className="inline">./.deepseek</code> 路径仍会作为兼容回退读取。
            </>
          ) : (
            <>
              The project-scoped <code className="inline">./.codewhale/</code> directory is optional —
              each repo can carry its own MCP servers, hooks, skills, and config overrides (e.g.
              provider keys). On first run the app asks whether to interactively create a config
              file if one is missing. Legacy <code className="inline">~/.deepseek</code> and{" "}
              <code className="inline">./.deepseek</code> paths are still read as compatibility fallbacks.
            </>
          )}
        </p>
      </section>

      {/* ⑦ PROVENANCE */}
      <section className="mx-auto max-w-[1100px] px-6 py-12 hairline-t">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="源" />
          <div className="eyebrow">{isZh ? "07 · 来源与镜像" : "07 · Provenance & mirrors"}</div>
        </div>

        <div className="space-y-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          <p>
            {isZh ? (
              <>
                <strong className="text-ink">codewhale.net</strong> 和{" "}
                <strong className="text-ink">www.codewhale.net</strong> 是 CodeWhale 的官方站点，
                部署在 Cloudflare 上。网站源码位于{" "}
                <code className="inline">Hmbown/CodeWhale</code> 仓库的{" "}
                <code className="inline">web/</code> 目录下，任何人都可自行部署为镜像。
              </>
            ) : (
              <>
                <strong className="text-ink">codewhale.net</strong> and{" "}
                <strong className="text-ink">www.codewhale.net</strong> are the official CodeWhale
                sites, deployed on Cloudflare. The website source lives under{" "}
                <code className="inline">web/</code> in the{" "}
                <code className="inline">Hmbown/CodeWhale</code> repository — anyone can
                self-deploy it as a mirror.
              </>
            )}
          </p>

          <div className="grid sm:grid-cols-2 gap-4 mt-4">
            <div>
              <div className="eyebrow mb-1 text-indigo">{isZh ? "官方发布" : "Official releases"}</div>
              <p>
                {isZh
                  ? "所有正式发布和 SHA-256 校验文件仅通过 GitHub Releases 分发。npm 包从 GitHub Releases 下载经校验的二进制。"
                  : "All official releases and SHA-256 checksums are distributed exclusively through GitHub Releases. The npm package downloads verified binaries from GitHub Releases."}
              </p>
            </div>
            <div>
              <div className="eyebrow mb-1 text-indigo">{isZh ? "CNB 镜像" : "CNB mirror"}</div>
              <p>
                {isZh ? (
                  <>
                    面向无法稳定访问 GitHub 的用户，提供 CNB 镜像（
                    <Link href="/docs#cnb-mirror" className="body-link">docs/CNB_MIRROR.md</Link>
                    ）。镜像仓库由社区成员维护，发布延迟可能为几小时。
                  </>
                ) : (
                  <>
                    A CNB mirror is available for users who cannot reliably reach GitHub (
                    <Link href="/docs#cnb-mirror" className="body-link">docs/CNB_MIRROR.md</Link>
                    ). The mirror is maintained by community members; release latency may be a few hours.
                  </>
                )}
              </p>
            </div>
            <div>
              <div className="eyebrow mb-1 text-indigo">{isZh ? "TUNA / 包镜像" : "TUNA / package mirrors"}</div>
              <p>
                {isZh
                  ? "Cargo 用户可通过 TUNA（清华大学开源镜像站）加速下载。这些镜像由第三方维护，CodeWhale 项目不控制镜像内容。"
                  : "Cargo users can accelerate downloads via TUNA (Tsinghua University Open Source Mirror). These mirrors are maintained by third parties; the CodeWhale project does not control mirror content."}
              </p>
            </div>
            <div>
              <div className="eyebrow mb-1 text-indigo">{isZh ? "自行部署" : "Self-deployed"}</div>
              <p>
                {isZh
                  ? "自行部署的网站副本、镜像站和第三方包不受 CodeWhale 项目控制。请验证下载来源和校验和。"
                  : "Self-deployed website copies, mirror sites, and third-party packages are not controlled by the CodeWhale project. Verify download sources and checksums."}
              </p>
            </div>
          </div>
        </div>
      </section>

      {/* ⑧ NEXT STEPS */}
      <section className="bg-paper-deep hairline-t hairline-b">
        <div className="mx-auto max-w-[1100px] px-6 py-12">
          <div className="flex items-baseline gap-4 mb-5">
            <Seal char="续" />
            <div className="eyebrow">{isZh ? "08 · 下一步" : "08 · Next steps"}</div>
          </div>
          <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b">
            <Link
              href={isZh ? "/zh/docs" : "/docs"}
              className="p-6 hover:bg-paper-deep transition-colors"
            >
              <div className="font-display text-xl mb-2">Docs</div>
              <div className="text-sm text-ink-soft mb-3">
                {isZh ? "模式、工具、配置、提供商、MCP" : "Modes, tools, config, providers, MCP"}
              </div>
              <span className="font-mono text-[0.7rem] uppercase tracking-widest text-indigo">
                {isZh ? "阅读文档 →" : "Read docs →"}
              </span>
            </Link>
            <Link
              href={isZh ? "/zh/faq" : "/faq"}
              className="p-6 hover:bg-paper-deep transition-colors"
            >
              <div className="font-display text-xl mb-2">FAQ</div>
              <div className="text-sm text-ink-soft mb-3">
                {isZh ? "安装、配置、模型、提供商等常见问题" : "Common questions on install, config, models, providers"}
              </div>
              <span className="font-mono text-[0.7rem] uppercase tracking-widest text-indigo">
                {isZh ? "查看 FAQ →" : "See FAQ →"}
              </span>
            </Link>
            <Link
              href={isZh ? "/zh/roadmap" : "/roadmap"}
              className="p-6 hover:bg-paper-deep transition-colors"
            >
              <div className="font-display text-xl mb-2">Roadmap</div>
              <div className="text-sm text-ink-soft mb-3">
                {isZh ? "已发布、进行中、考虑中、暂不考虑" : "Shipped, underway, considered, ruled out"}
              </div>
              <span className="font-mono text-[0.7rem] uppercase tracking-widest text-indigo">
                {isZh ? "查看路线图 →" : "View roadmap →"}
              </span>
            </Link>
          </div>
        </div>
      </section>
    </>
  );
}
