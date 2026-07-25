import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/configuration",
    locale,
    title: isZh ? "配置 · Codewhale 文档" : "Configuration · Codewhale Docs",
    description: isZh
      ? "config.toml 的查找顺序、项目级覆盖、凭据优先级和旧版路径迁移。"
      : "Where config.toml is read from, the per-project overlay, credential precedence, and legacy path migration.",
  });
}

export default async function ConfigurationPage({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "配置" : "Configuration"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 从 ~/.codewhale/config.toml 读取配置（旧版 ~/.deepseek/config.toml 仍作为回退读取）。--config 标志和 CODEWHALE_CONFIG_PATH 环境变量可以指定别的路径，两者同时设置时 --config 优先；文件加载之后再应用环境变量覆盖。"
            : "Codewhale reads its configuration from ~/.codewhale/config.toml (the legacy ~/.deepseek/config.toml is still read as a fallback). The --config flag and the CODEWHALE_CONFIG_PATH environment variable can point elsewhere; --config wins when both are set, and environment variable overrides are applied after the file is loaded."}
        </p>
        <pre className="code-block mt-4">{`codewhale --config /path/to/config.toml
CODEWHALE_CONFIG_PATH=/path/to/config.toml`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              在 TUI 里运行 <code className="inline">/config audit</code>{" "}
              可以查看哪些文档化的键能在当前会话修改、哪些能持久化、哪些只能改文件或需要重启——改动前以它输出的
              “Command / reason” 列为准。
            </>
          ) : (
            <>
              Inside the TUI, <code className="inline">/config audit</code> shows which documented keys
              can change in the current session, which can also be persisted, and which stay file-only or
              restart-only — treat its “Command / reason” column as the source of truth before editing by
              hand.
            </>
          )}
        </p>
      </section>

      <section id="project-overlay" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "项目级覆盖" : "Per-project overlay"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "当工作区包含常规文件 <workspace>/.codewhale/config.toml 时，其中声明的安全取值会合并到全局配置之上（旧版 <workspace>/.deepseek/config.toml 在新路径缺失时仍会读取；符号链接的项目配置会被拒绝）。这让仓库可以建议模型或收紧本地安全姿态，而不动用户的全局配置。单次启动可用 --no-project-config 跳过覆盖。"
            : "When a workspace contains a regular-file <workspace>/.codewhale/config.toml, the safe values it declares are merged on top of the global config (legacy <workspace>/.deepseek/config.toml files are still read when the Codewhale path is absent; symlinked project configs are rejected). This lets a repository suggest a model or tighten the local safety posture without touching the user's global config. Pass --no-project-config to skip the overlay for one launch."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "覆盖层有意保持狭窄：支持 model、reasoning_effort、approval_policy 与 sandbox_mode（只能收紧）、notes_path、max_subagents（夹紧到 1..=20）、allow_shell（false 生效，true 被忽略）。凭据、端点、提供商选择、MCP 配置、hooks、skills 和 instructions = [...] 始终属于用户全局配置——仓库里的 config.toml 声明 api_key、base_url 或 provider 会被忽略，克隆的仓库无法借此选择任意本地文件进入提示词。"
            : "The overlay is intentionally narrow: it supports model, reasoning_effort, approval_policy and sandbox_mode (tightening values only), notes_path, max_subagents (clamped to 1..=20), and allow_shell (false applies, true is ignored). Credentials, endpoints, provider selection, MCP config, hooks, skills, and instructions = [...] stay user-global — a repo-local config.toml that declares api_key, base_url, or provider is ignored, so a cloned repository cannot pick arbitrary local files into the prompt."}
        </p>
      </section>

      <section id="credentials" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "凭据查找" : "Credential lookup"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              在显式 <code className="inline">--api-key</code> 之后，凭据按 config → keyring → env
              的顺序解析。<code className="inline">codewhale auth status</code>{" "}
              可以查看当前提供商的配置文件、系统 keyring 后端、环境变量、生效来源和末四位标签，而不会打印密钥本身。托管、OpenAI 兼容、自托管或 Anthropic 原生路由用{" "}
              <code className="inline">{"provider = \"<id>\""}</code> 或{" "}
              <code className="inline">codewhale --provider &lt;id&gt;</code>{" "}
              选择；完整注册表见模型与提供商页和 docs/PROVIDERS.md。
            </>
          ) : (
            <>
              After any explicit <code className="inline">--api-key</code>, credentials resolve in
              config → keyring → env order. <code className="inline">codewhale auth status</code>{" "}
              inspects the active provider's config file, OS keyring backend, environment variable,
              winning source, and last-four label without printing the key itself. Hosted, generic
              OpenAI-compatible, self-hosted, or native Anthropic routes are selected with{" "}
              <code className="inline">{"provider = \"<id>\""}</code> or{" "}
              <code className="inline">codewhale --provider &lt;id&gt;</code>; the full registry lives on
              the Models &amp; providers page and in docs/PROVIDERS.md.
            </>
          )}
        </p>
      </section>

      <section id="legacy-paths" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "旧版 .deepseek/ 路径" : "Legacy .deepseek/ paths"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 由 DeepSeek-TUI 更名而来。为了不破坏既有安装，运行时从新的 ~/.codewhale/ 位置读取状态，但在只有旧目录存在时回退到 ~/.deepseek/，并且始终写入 ~/.codewhale/——读取带回退、写入新位置。状态目录解析集中在 crates/config/src/lib.rs 的 resolve_state_dir / ensure_state_dir 中，每一处旧路径引用都有审计过的保留决定。"
            : "Codewhale was renamed from DeepSeek-TUI. To avoid breaking existing installs, the runtime reads state from the new ~/.codewhale/ location but falls back to ~/.deepseek/ when only the legacy directory exists, and always writes to ~/.codewhale/ — read-with-fallback, write-to-new. State-dir resolution is consolidated in resolve_state_dir / ensure_state_dir in crates/config/src/lib.rs, and every legacy path reference carries an audited keep decision."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/CONFIGURATION.md, docs/LEGACY_PATHS.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/CONFIGURATION.md, docs/LEGACY_PATHS.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
