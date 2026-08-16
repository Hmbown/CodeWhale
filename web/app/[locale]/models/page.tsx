import Link from "next/link";
import { getFacts } from "@/lib/facts";
import { buildPageMetadata } from "@/lib/page-meta";

export const revalidate = 300;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/models",
    locale,
    title: isZh ? "模型与提供商 · Codewhale" : "Models & providers · Codewhale",
    description: isZh
      ? "Codewhale 托管与本地提供商路由的配置方式和完整注册表。"
      : "Configuration guidance and the full registry for Codewhale's hosted and local provider routes.",
  });
}

export default async function ModelsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const p = (path: string) => (isZh ? `/zh${path}` : `/en${path}`);
  const facts = await getFacts();
  const providerDocs = "https://github.com/Hmbown/CodeWhale/blob/main/docs/PROVIDERS.md";

  const setupPatterns = isZh
    ? [
        {
          title: "DeepSeek",
          detail: `新配置默认使用 ${facts.defaultModel ?? "deepseek-v4-pro"}。可以通过 --provider、/provider 或 CODEWHALE_PROVIDER 明确选择其他路由。`,
          reference: "DEEPSEEK_API_KEY",
        },
        {
          title: "本地运行时",
          detail: "vLLM、SGLang 和 Ollama 可以直连 localhost。按需配置端点和模型；本地部署通常不需要 API 密钥。",
          reference: "vllm · sglang · ollama",
        },
        {
          title: "OpenRouter",
          detail: "OpenRouter 用一个托管端点访问多个模型。提供商和模型仍由你明确选择，不会根据模型名称或提示词自动切换。",
          reference: "OPENROUTER_API_KEY",
        },
      ]
    : [
        {
          title: "DeepSeek",
          detail: `New configurations default to ${facts.defaultModel ?? "deepseek-v4-pro"}. Select another route explicitly with --provider, /provider, or CODEWHALE_PROVIDER.`,
          reference: "DEEPSEEK_API_KEY",
        },
        {
          title: "Local runtimes",
          detail: "vLLM, SGLang, and Ollama can connect directly to localhost. Set an endpoint and model as needed; local deployments usually require no API key.",
          reference: "vllm · sglang · ollama",
        },
        {
          title: "OpenRouter",
          detail: "OpenRouter provides one hosted endpoint for many models. You still select the provider and model explicitly; model names and prompts do not switch routes.",
          reference: "OPENROUTER_API_KEY",
        },
      ];

  return (
    <div className="models-page">
      <section className="hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container community-welcome-inner">
          <div className="eyebrow">{isZh ? "模型与提供商" : "Models and providers"}</div>
          <h1>{isZh ? "选择模型和提供商。" : "Choose a model and provider."}</h1>
          <p>
            {isZh
              ? `Codewhale 包含 ${facts.providers.length} 条提供商路由。提供商、模型和端点都是明确的配置；每条路由使用同一个本地运行时、工具和审批边界。托管提供商使用你配置的凭据，本地 vLLM、SGLang 和 Ollama 端点通常不需要密钥。`
              : `Codewhale includes ${facts.providers.length} provider routes. The provider, model, and endpoint are explicit configuration, and every route uses the same local runtime, tools, and approval boundaries. Hosted providers use credentials you configure; local vLLM, SGLang, and Ollama endpoints usually require no key.`}
          </p>
          <div className="portal-actions">
            <Link href={providerDocs} className="portal-button portal-button-primary">
              {isZh ? "阅读提供商文档" : "Read the provider docs"}
            </Link>
            <Link href={p("/install")} className="portal-button portal-button-secondary">
              {isZh ? "安装 Codewhale" : "Install Codewhale"}
            </Link>
          </div>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>{isZh ? "配置方式" : "Configuration paths"}</span>
            <h2>{isZh ? "常用提供商路由" : "Common provider routes"}</h2>
            <p>
              {isZh
                ? "托管提供商的密钥可以通过 codewhale auth set 保存，也可以使用文档中列出的配置项或环境变量。提供商和模型分别选择；模型名称不会隐式改变提供商。"
                : "Hosted-provider credentials can be saved with codewhale auth set or supplied through documented configuration and environment variables. Provider and model selection remain separate; a model name never changes the provider implicitly."}
            </p>
          </div>
          <div className="portal-topic-list">
            {setupPatterns.map((pattern) => (
              <Link key={pattern.title} href={p("/docs/configuration")}>
                <strong>{pattern.title}</strong>
                <span>{pattern.detail}</span>
                <span className="font-mono break-all">{pattern.reference}</span>
              </Link>
            ))}
          </div>
        </div>
      </section>

      <section className="portal-section settings-preview" aria-labelledby="settings-preview-title">
        <div className="portal-container">
          <div className="settings-preview-heading">
            <div>
              <span>{isZh ? "设置界面" : "Settings surface"}</span>
              <h2 id="settings-preview-title">
                {isZh ? "只读设置预览" : "Read-only settings preview"}
              </h2>
            </div>
            <p>
              {isZh
                ? "这是 Codewhale 本地配置界面的只读说明，不会更改你的本地配置。"
                : "This is a read-only guide to Codewhale's local configuration and does not change your local configuration."}
            </p>
          </div>

          <div className="settings-shell">
            <aside className="settings-rail" aria-label={isZh ? "设置区域" : "Settings areas"}>
              <div className="settings-rail-title">{isZh ? "设置" : "Settings"}</div>
              <ul>
                <li className="settings-rail-item settings-rail-item-active">
                  <span>{isZh ? "模型与提供商" : "Models & providers"}</span>
                  <span>{isZh ? "当前" : "Current"}</span>
                </li>
                <li className="settings-rail-item">{isZh ? "运行时" : "Runtime"}</li>
                <li className="settings-rail-item">{isZh ? "模式" : "Modes"}</li>
                <li className="settings-rail-item">{isZh ? "权限" : "Permissions"}</li>
                <li className="settings-rail-item">{isZh ? "工具与 MCP" : "Tools & MCP"}</li>
              </ul>
            </aside>

            <div className="settings-pane">
              <div className="settings-pane-heading">
                <div>
                  <span>{isZh ? "本地工具链" : "Local harness"}</span>
                  <h3>{isZh ? "模型与提供商" : "Models & providers"}</h3>
                </div>
                <span className="settings-readonly-badge">{isZh ? "只读" : "Read only"}</span>
              </div>

              <dl className="settings-default-model">
                <div>
                  <dt>{isZh ? "默认模型" : "Default model"}</dt>
                  <dd>
                    <code className="settings-provider-code">{facts.defaultModel ?? "—"}</code>
                  </dd>
                </div>
                <div>
                  <dt>{isZh ? "提供商路由" : "Provider routes"}</dt>
                  <dd>{facts.providers.length}</dd>
                </div>
              </dl>

              <div className="settings-provider-heading">
                <span>{isZh ? "仓库注册表" : "Repository registry"}</span>
                <span>{isZh ? "认证环境变量" : "Authentication environment"}</span>
              </div>
              <ul className="settings-provider-list">
                {facts.providers.map((provider) => (
                  <li key={provider.id}>
                    <div>
                      <strong>{provider.label}</strong>
                      <code className="settings-provider-code">{provider.id}</code>
                    </div>
                    <div className="settings-provider-auth">
                      <span className="settings-registry-marker" aria-hidden="true" />
                      <code className="settings-provider-code">{provider.env}</code>
                    </div>
                  </li>
                ))}
              </ul>

              <div className="settings-docs-action">
                <p>
                  {isZh
                    ? "要在你的机器上选择路由、模型、端点或凭据，请按照配置文档操作。"
                    : "To choose a route, model, endpoint, or credentials on your machine, follow the configuration documentation."}
                </p>
                <Link href={p("/docs/configuration")}>
                  {isZh ? "打开配置文档 ↗" : "Open configuration docs ↗"}
                </Link>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section className="portal-section portal-section-muted">
        <div className="portal-container">
          <div className="portal-docs-heading">
            <div>
              <span>{isZh ? "仓库数据" : "Repository data"}</span>
              <h2>{isZh ? "内置提供商注册表" : "Built-in provider registry"}</h2>
            </div>
            <Link href={providerDocs}>{isZh ? "打开源文档 ↗" : "Open the source document ↗"}</Link>
          </div>
          <p className={`mb-6 max-w-3xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
            {isZh
              ? "下面的列表由仓库中的提供商注册表生成，并随发布更新。这里列出路由 ID 和常用认证环境变量；传输协议、默认端点、模型解析和完整认证优先级以 docs/PROVIDERS.md 为准。"
              : "This list is generated from the provider registry in the repository and updated with releases. It shows route IDs and common authentication environment variables; docs/PROVIDERS.md is the source for wire protocols, default endpoints, model resolution, and full authentication precedence."}
          </p>
          <ul className="grid gap-3 sm:grid-cols-2">
            {facts.providers.map((provider) => (
              <li key={provider.id} className="flex items-start gap-3 border hairline rounded-lg bg-paper px-4 py-3 min-w-0">
                <div className="min-w-0">
                  <div className="text-sm text-ink font-medium">{provider.label}</div>
                  <code className="font-mono text-[0.66rem] text-indigo break-all">{provider.id}</code>
                  <div className="mt-1 font-mono text-[0.62rem] text-ink-mute break-all leading-relaxed">
                    <code className="inline">{provider.env}</code>
                  </div>
                </div>
              </li>
            ))}
          </ul>
          <p className={`mt-6 max-w-3xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
            {isZh ? (
              <>
                如果需要的提供商尚未列出，请先{" "}
                <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose" className="body-link">提交 issue</Link>
                ，说明端点、认证方式和模型能力；也欢迎发送包含注册表、文档和测试的 pull request。
              </>
            ) : (
              <>
                If a provider is missing, please{" "}
                <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose" className="body-link">file an issue</Link>
                {" "}with its endpoint, authentication method, and model capabilities. Pull requests that update the registry, documentation, and tests are welcome too.
              </>
            )}
          </p>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container">
          <div className="portal-docs-heading">
            <div>
              <span>{isZh ? "模型目录" : "Model catalog"}</span>
              <h2>{isZh ? "默认模型与 crate 清单" : "Default model & crate inventory"}</h2>
            </div>
          </div>
          <div className="grid gap-8 sm:grid-cols-2">
            <div>
              <div className="eyebrow mb-1">{isZh ? "默认模型" : "Default model"}</div>
              <code className="inline font-mono text-sm break-all">{facts.defaultModel ?? "—"}</code>
            </div>
            <div>
              <div className="eyebrow mb-1">{isZh ? "Crates" : "Crates"}</div>
              <ul className="flex flex-wrap gap-1.5">
                {facts.crates.map((crate) => (
                  <li key={crate}>
                    <code className="inline font-mono text-[0.68rem] break-all">{crate}</code>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
