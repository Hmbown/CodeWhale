import Link from "next/link";
import { Seal } from "@/components/seal";
import { getFacts } from "@/lib/facts";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/models",
    locale,
    title: isZh ? "模型与提供商 · CodeWhale" : "Models & providers · CodeWhale",
    description: isZh
      ? "自带密钥，没有推理加价，不会悄悄换模型。DeepSeek 一级支持，本地 vLLM / SGLang / Ollama 无需密钥，所有提供商共用同一个运行时和同一套工具。"
      : "Bring your own key, no inference markup, no silent model switching. DeepSeek is first-class, local vLLM / SGLang / Ollama need no key, and every provider routes through the same runtime and tools.",
  });
}

/** Provider ids covered by the featured cards; everything else renders as a peer card. */
const FEATURED_IDS = new Set(["deepseek", "deepseek-anthropic", "vllm", "sglang", "ollama", "openrouter"]);

export default async function ModelsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const p = (path: string) => (isZh ? `/zh${path}` : `/en${path}`);
  const facts = await getFacts();

  const peers = facts.providers.filter((prov) => !FEATURED_IDS.has(prov.id));

  return (
    <>
      {/* THE FRAMING */}
      <section className="mx-auto max-w-[1100px] px-6 pt-12 pb-10">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="模" />
          <div className="eyebrow">{isZh ? "模型与提供商" : "Models & providers"}</div>
        </div>
        <h1 className="font-display tracking-crisp mb-6">
          {isZh ? "任意模型，实话实说" : "Any model, honestly"}
        </h1>
        <p className={`max-w-2xl text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? `${facts.providers.length} 个提供商，全部经由同一个运行时、同一套工具。宪法与安全边界住在执行框架里，不在模型里——所以换提供商不是换产品。`
            : `${facts.providers.length} providers, and every one routes through the same runtime and the same tools. The constitution and safety boundaries live in the harness, not the model — so changing providers doesn't change the product.`}
        </p>

        {/* The honest terms */}
        <div className="mt-8 grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b">
          {(isZh
            ? [
                { t: "自带密钥", d: "codewhale auth set --provider … 把密钥存进本机的 ~/.codewhale/config.toml。请求直达你配置的提供商。" },
                { t: "没有推理加价", d: "CodeWhale 不经手计费：没有中转、没有转售。账单在你和提供商之间，跟这个项目无关。" },
                { t: "不会悄悄换模型", d: "提供商和模型是你显式设定的路由，不从提示词里猜。换路由是你亲手敲的命令：/provider 和 /model。" },
              ]
            : [
                { t: "Bring your own key", d: "codewhale auth set --provider … stores keys in your local ~/.codewhale/config.toml. Requests go straight to the provider you configured." },
                { t: "No inference markup", d: "CodeWhale never sits in the billing path — no relay, no resale. The bill is between you and your provider; this project isn't on it." },
                { t: "No silent model switching", d: "The provider and model are an explicit route you set, not inferred from a prompt. Changing it is a command you type: /provider and /model." },
              ]
          ).map((item) => (
            <div key={item.t} className="p-6">
              <h2 className="font-display text-xl mb-2">{item.t}</h2>
              <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {item.d}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* FEATURED ROUTES */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-6">
          <Seal char="先" />
          <div className="eyebrow">{isZh ? "先说这几条路" : "Start with these routes"}</div>
        </div>

        <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b">
          {/* DeepSeek — first-class and the default */}
          <div className="p-6">
            <div className="flex items-baseline gap-2 mb-2">
              <h2 className="font-display text-xl">DeepSeek</h2>
              <span className="pill pill-hot text-[0.58rem]">{isZh ? "一级支持 · 默认" : "first-class · default"}</span>
            </div>
            <div className="font-mono text-[0.68rem] text-ink-mute mb-3 break-all">DEEPSEEK_API_KEY</div>
            <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
              {isZh
                ? `默认提供商，默认模型 ${facts.defaultModel ?? "deepseek-v4-pro"}。DeepSeek API（api.deepseek.com）中国大陆直连，无需代理。另有 deepseek-anthropic——DeepSeek 可选的 Messages-API 路由。`
                : `The default provider, with ${facts.defaultModel ?? "deepseek-v4-pro"} as the default model. The DeepSeek API (api.deepseek.com) is reachable from mainland China without a proxy. There's also deepseek-anthropic, DeepSeek's opt-in Messages-API route.`}
            </p>
          </div>

          {/* Local runtimes — no key at all */}
          <div className="p-6">
            <div className="flex items-baseline gap-2 mb-2">
              <h2 className="font-display text-xl">{isZh ? "本地运行时" : "Local runtimes"}</h2>
              <span className="pill pill-jade text-[0.58rem]">{isZh ? "无需密钥" : "no key at all"}</span>
            </div>
            <div className="font-mono text-[0.68rem] text-ink-mute mb-3 break-all">vllm · sglang · ollama</div>
            <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
              {isZh
                ? "vLLM、SGLang、Ollama——指向你自己的 localhost 端点即可，完全不需要密钥。权重在你的机器上，请求也不出你的机器。"
                : "vLLM, SGLang, and Ollama against your own localhost endpoints — no key required. The weights are on your machine, and the requests never leave it."}
            </p>
          </div>

          {/* OpenRouter — one key, many models */}
          <div className="p-6">
            <div className="flex items-baseline gap-2 mb-2">
              <h2 className="font-display text-xl">OpenRouter</h2>
              <span className="pill pill-ghost text-[0.58rem]">{isZh ? "一把密钥" : "one key"}</span>
            </div>
            <div className="font-mono text-[0.68rem] text-ink-mute mb-3 break-all">OPENROUTER_API_KEY</div>
            <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
              {isZh
                ? "一把密钥接入众多托管模型，想把开放模型挨个试一遍时最省事。路由仍然由你显式指定——OpenRouter 换的是端点，不是规则。"
                : "One key that reaches many hosted models — the easiest way to try open models back to back. The route is still yours to set explicitly; OpenRouter changes the endpoint, not the rules."}
            </p>
          </div>
        </div>
      </section>

      {/* THE REST OF THE REGISTRY */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="众" />
          <div className="eyebrow">{isZh ? "其余提供商，一视同仁" : "The rest, as peers"}</div>
        </div>
        <p className={`mb-6 max-w-2xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "这份列表由仓库的提供商注册表生成，随发布同步。托管的、闭源的、实验性的，都走同一条审批、沙箱与回滚流水线。"
            : "This list is generated from the repo's provider registry and tracks releases. Hosted, closed, or experimental — all of them go through the same approval, sandbox, and rollback pipeline."}
        </p>

        <div className="grid sm:grid-cols-2 lg:grid-cols-3 gap-0 hairline-t hairline-l">
          {peers.map((prov) => (
            <div key={prov.id} className="p-4 hairline-b hairline-r">
              <div className="font-display text-base mb-1">{prov.label}</div>
              <div className="font-mono text-[0.66rem] text-indigo mb-1">{prov.id}</div>
              <div className="font-mono text-[0.62rem] text-ink-mute break-all leading-relaxed">{prov.env}</div>
            </div>
          ))}
        </div>

        <p className={`mt-6 max-w-2xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh ? (
            <>
              想要的提供商不在这里？<a href="https://github.com/Hmbown/CodeWhale/issues/new" className="body-link">这正是值得开的 issue</a>。凭据、base URL 和能力边界的完整注册表见{" "}
              <a href="https://github.com/Hmbown/CodeWhale/blob/main/docs/PROVIDERS.md" className="body-link">docs/PROVIDERS.md</a>。
            </>
          ) : (
            <>
              Don&apos;t see the provider you want? <a href="https://github.com/Hmbown/CodeWhale/issues/new" className="body-link">That&apos;s a good issue to open</a>. The full registry — credentials, base URLs, capability boundaries — lives in{" "}
              <a href="https://github.com/Hmbown/CodeWhale/blob/main/docs/PROVIDERS.md" className="body-link">docs/PROVIDERS.md</a>.
            </>
          )}
        </p>
      </section>

      {/* WHERE TO GO NEXT */}
      <section className="mx-auto max-w-[1100px] px-6 py-8 hairline-t">
        <div className="flex flex-wrap items-center gap-3">
          <Link
            href={p("/install")}
            className="px-5 py-3 bg-ink text-paper font-mono text-sm uppercase tracking-wider hover:bg-indigo transition-colors"
          >
            {isZh ? "安装 →" : "Install →"}
          </Link>
          <Link
            href={p("/docs#providers")}
            className="px-5 py-3 hairline-t hairline-b hairline-l hairline-r font-mono text-sm uppercase tracking-wider hover:bg-paper-deep transition-colors"
          >
            {isZh ? "配置密钥：文档 →" : "Key setup: docs →"}
          </Link>
        </div>
      </section>
    </>
  );
}
