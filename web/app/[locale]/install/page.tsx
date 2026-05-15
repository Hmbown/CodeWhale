import Link from "next/link";
import { Seal } from "@/components/seal";
import { InstallCodeBlock } from "@/components/install-code-block";
import { InstallBinary } from "@/components/install-binary";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "安装 · DeepSeek TUI" : "Install · DeepSeek TUI",
    description: isZh
      ? "通过 Cargo 安装 deepseek-tui。其他方式：npm、Homebrew、预编译二进制、Docker、国内镜像。"
      : "Install deepseek-tui via Cargo. Other ways: npm, Homebrew, prebuilt binary, Docker, source.",
  };
}

const CARGO_INSTALL = `cargo install deepseek-tui-cli --locked`;
const FIRST_RUN = `deepseek`;
const VERIFY = `deepseek --version
deepseek doctor`;

const UPDATE = `deepseek update`;

const SET_KEY_BASH = `export DEEPSEEK_API_KEY=sk-...`;
const SET_KEY_AUTH = `deepseek auth set --provider deepseek --api-key sk-...`;

const NPM_INSTALL = `npm install -g deepseek-tui`;

const TUNA_CONFIG = `# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"`;
const TUNA_INSTALL = `cargo install deepseek-tui-cli --locked`;
const NPMMIRROR = `npm config set registry https://registry.npmmirror.com
npm install -g deepseek-tui`;

const BREW = `brew tap Hmbown/deepseek-tui
brew install deepseek-tui`;

const DOCKER = `git clone https://github.com/Hmbown/deepseek-tui
cd deepseek-tui
docker build -t deepseek-tui .

docker run --rm -it \\
  -e DEEPSEEK_API_KEY=$DEEPSEEK_API_KEY \\
  -v ~/.deepseek:/home/deepseek/.deepseek \\
  -v "$PWD:/work" -w /work \\
  deepseek-tui`;

const FROM_SOURCE = `git clone https://github.com/Hmbown/deepseek-tui
cd deepseek-tui
cargo build --release --locked

# Install both binaries from the local checkout
cargo install --path crates/cli --locked   # deepseek
cargo install --path crates/tui --locked   # deepseek-tui`;

const CONFIG_TREE = `~/.deepseek/
├── config.toml      api keys, model, hooks, profiles
├── mcp.json         MCP server definitions
├── skills/          user skills (each with SKILL.md)
├── sessions/        checkpoints + offline queue
├── tasks/           background task store
└── audit.log        credential / approval / elevation audit trail

./.deepseek/         project-scoped config (optional, per-repo)`;

const CONFIG_TREE_ZH = `~/.deepseek/
├── config.toml      API 密钥、模型、钩子、配置集
├── mcp.json         MCP 服务器定义
├── skills/          用户技能（每个含 SKILL.md）
├── sessions/        检查点 + 离线队列
├── tasks/           后台任务存储
└── audit.log        凭证 / 审批 / 提权审计日志

./.deepseek/         项目级配置（可选，每个仓库）`;

export default async function InstallPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

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
          <InstallCodeBlock cmd={CARGO_INSTALL} copyLabel={copyLabel} copiedLabel={copiedLabel} />
          <InstallCodeBlock cmd={FIRST_RUN} copyLabel={copyLabel} copiedLabel={copiedLabel} />
        </div>

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              编译并安装 <code className="inline">deepseek</code> 到 <code className="inline">~/.cargo/bin</code>。
              需要 Rust 1.88+——如未安装可访问{" "}
              <a href="https://rustup.rs" className="body-link">rustup.rs</a>。
              下方「其他安装方式」列出了不用 Rust 工具链、国内镜像、Homebrew、预编译二进制等替代选项。
            </>
          ) : (
            <>
              Compiles and installs <code className="inline">deepseek</code> to{" "}
              <code className="inline">~/.cargo/bin</code>. Requires Rust 1.88+ — install via{" "}
              <a href="https://rustup.rs" className="body-link">rustup.rs</a> if you don&apos;t have it.
              See <a href="#other-ways" className="body-link">Other ways to install</a> below for
              npm, Homebrew, prebuilt binaries, or mainland China mirrors.
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

        <InstallCodeBlock cmd={VERIFY} copyLabel={copyLabel} copiedLabel={copiedLabel} />

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              <code className="inline">deepseek doctor</code> 检查 API 密钥、网络、沙箱可用性、
              MCP 服务器，并将完整报告写入{" "}
              <code className="inline">~/.deepseek/doctor.log</code>。
            </>
          ) : (
            <>
              <code className="inline">deepseek doctor</code> checks your API key, network,
              sandbox availability, and MCP servers. Full report is written to{" "}
              <code className="inline">~/.deepseek/doctor.log</code>.
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
              通过 Homebrew 或 npm 安装的话，使用包管理器升级更稳：
              <code className="inline">brew upgrade deepseek-tui</code> 或{" "}
              <code className="inline">npm update -g deepseek-tui</code>。
              Cargo 安装的可以重跑{" "}
              <code className="inline">cargo install deepseek-tui-cli --locked --force</code>。
            </>
          ) : (
            <>
              Checks GitHub Releases for a newer version and replaces the binary in place. If you
              installed via Homebrew or npm, prefer the package manager instead:{" "}
              <code className="inline">brew upgrade deepseek-tui</code> or{" "}
              <code className="inline">npm update -g deepseek-tui</code>. Cargo users can re-run{" "}
              <code className="inline">cargo install deepseek-tui-cli --locked --force</code>.
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
                {isZh ? "或保存到 ~/.deepseek/config.toml：" : "Or persist it to ~/.deepseek/config.toml:"}
              </p>
              <InstallCodeBlock cmd={SET_KEY_AUTH} copyLabel={copyLabel} copiedLabel={copiedLabel} />
            </div>
          </li>

          <li>
            <div className="font-display text-lg mb-2">
              {isZh ? "③ 在项目目录中运行" : "③ Run it in a project"}
            </div>
            <InstallCodeBlock cmd={`cd path/to/project\ndeepseek`} copyLabel={copyLabel} copiedLabel={copiedLabel} />
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
              ? "如果上面的 Cargo 路径不适合你，从下面找到匹配你情况的一条。每条都安装同一个 deepseek 二进制。"
              : "If the Cargo path above doesn't fit your setup, pick the row that matches your situation. Every path installs the same deepseek binary."}
          </p>

          <div className="space-y-10">
            {/* No Rust toolchain */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                {isZh ? "没有 Rust 工具链" : "No Rust toolchain"}
              </div>
              <InstallCodeBlock cmd={NPM_INSTALL} copyLabel={copyLabel} copiedLabel={copiedLabel} />
              <p className="mt-3 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh ? (
                  <>
                    npm 包装器会从 GitHub Releases 下载对应平台的预编译二进制。需要 Node 18+。
                    安装后会同时提供 <code className="inline">deepseek</code> 和{" "}
                    <code className="inline">deepseek-tui</code> 两个命令。
                  </>
                ) : (
                  <>
                    The npm wrapper downloads the prebuilt binary from GitHub Releases for your
                    platform. Requires Node 18+. Installs both <code className="inline">deepseek</code>{" "}
                    and <code className="inline">deepseek-tui</code> on PATH.
                  </>
                )}
              </p>
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

              <p className="text-sm text-ink-soft leading-relaxed max-w-2xl mt-6 mb-3">
                {isZh ? "npm 经 npmmirror 镜像：" : "npm via npmmirror:"}
              </p>
              <InstallCodeBlock cmd={NPMMIRROR} copyLabel={copyLabel} copiedLabel={copiedLabel} />

              <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
                {isZh ? (
                  <>
                    npm 包装器仍会从{" "}
                    <code className="inline">github.com/Hmbown/deepseek-tui/releases</code>{" "}
                    下载二进制，国内可能较慢。Cargo + Tuna 完全绕开 GitHub。
                    DeepSeek API（<code className="inline">api.deepseek.com</code>）在国内直连，无需代理。
                  </>
                ) : (
                  <>
                    The npm wrapper still downloads the binary from{" "}
                    <code className="inline">github.com/Hmbown/deepseek-tui/releases</code>, which can
                    be slow over GFW. Cargo + Tuna routes around GitHub entirely. The DeepSeek API
                    at <code className="inline">api.deepseek.com</code> is reachable from mainland
                    China without a proxy.
                  </>
                )}
              </p>
            </div>

            {/* Homebrew */}
            <div>
              <div className="eyebrow mb-2 text-indigo">
                Homebrew{" "}
                <span className="text-ink-mute font-mono normal-case tracking-normal">
                  · macOS / Linux
                </span>
              </div>
              <InstallCodeBlock cmd={BREW} copyLabel={copyLabel} copiedLabel={copiedLabel} />
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
                  ? "支持 multi-arch buildx。目前没有发布到镜像仓库，需要本地构建。"
                  : "Multi-arch buildx is supported. No image is published to a registry yet, so you build locally."}
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
                  : "Useful for hacking on the workspace itself or contributing patches."}
              </p>
            </div>
          </div>
        </div>
      </section>

      {/* ⑥ WHERE CONFIG LIVES */}
      <section className="mx-auto max-w-[1100px] px-6 py-12">
        <div className="flex items-baseline gap-4 mb-5">
          <Seal char="件" />
          <div className="eyebrow">
            {isZh ? "06 · 配置文件位置" : "06 · Where config lives"}
          </div>
        </div>
        <h2 className="font-display text-3xl mb-6">
          {isZh ? "配置文件位置" : "Where config lives"}
        </h2>

        <InstallCodeBlock
          cmd={isZh ? CONFIG_TREE_ZH : CONFIG_TREE}
          copyLabel={copyLabel}
          copiedLabel={copiedLabel}
        />

        <p className="mt-4 text-sm text-ink-soft leading-relaxed max-w-2xl">
          {isZh ? (
            <>
              所有用户配置存放在 <code className="inline">~/.deepseek/</code>。仓库根目录下的{" "}
              <code className="inline">.deepseek/</code> 用于项目级覆盖。
              完整字段参考{" "}
              <Link href={isZh ? "/zh/docs" : "/docs"} className="body-link">
                {isZh ? "文档" : "the docs"}
              </Link>
              。
            </>
          ) : (
            <>
              All user-level configuration goes under <code className="inline">~/.deepseek/</code>.
              Per-project overrides live in <code className="inline">.deepseek/</code> at the repo
              root. Full field reference in{" "}
              <Link href="/docs" className="body-link">the docs</Link>.
            </>
          )}
        </p>
      </section>
    </>
  );
}
