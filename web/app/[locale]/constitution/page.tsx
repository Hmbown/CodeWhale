import Link from "next/link";
import { Seal } from "@/components/seal";
import { ThinkingTrace } from "@/components/thinking-trace";
import { buildPageMetadata } from "@/lib/page-meta";
import { FACTS } from "@/lib/facts.generated";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/constitution",
    locale,
    title: isZh ? "三层法 · CodeWhale" : "Three layers of law · CodeWhale",
    description: isZh
      ? "CodeWhale 的嵌套宪法：内置基础法、你的常备法（/constitution）、仓库自己的法（.codewhale/constitution.json）。位阶由执行框架强制生效，换掉模型也不失效。"
      : "CodeWhale's nested constitution: bundled base law, your standing law (/constitution), and your repo's law (.codewhale/constitution.json). Rank is enforced in the harness and survives a model swap.",
  });
}

/** The three layers, rendered as a card row on this page and (compact) on the homepage. */
const LAYERS = [
  {
    n: "01",
    name: { en: "Bundled Constitution", zh: "内置宪法" },
    path: "compiled into every binary",
    pathZh: "编译进每一个二进制",
    en: "The base law. Its priority article fixes the authority order for any conflict, so a stale handoff can never outrank a fresh test result by accident.",
    zh: "基础法。其中的位阶条款为一切冲突固定裁决顺序——过期的交接不会稀里糊涂地压过刚跑出的测试结果。",
  },
  {
    n: "02",
    name: { en: "/constitution — your standing law", zh: "/constitution——你的常备法" },
    path: "$CODEWHALE_HOME/constitution.json",
    pathZh: "$CODEWHALE_HOME/constitution.json",
    en: "Structured data, not a raw prompt editor: guided setup renders it into a model-facing prose block. Drafted with your model's help, ratified by you, and carried across every project.",
    zh: "结构化数据，不是裸的提示词编辑器：引导式设置把它渲染成面向模型的 prose 区块。可由模型协助起草，经你批准生效，跨项目随身携带。",
  },
  {
    n: "03",
    name: { en: "Your repo's law", zh: "仓库自己的法" },
    path: ".codewhale/constitution.json",
    pathZh: ".codewhale/constitution.json",
    en: "Protected invariants, branch policy, verification requirements, escalation conditions — loaded as a repo-local authority block above project instructions, memory, and handoffs.",
    zh: "受保护的不变量、分支策略、验证要求、升级条件——作为仓库本地的权威区块加载，位阶高于项目说明、记忆与交接。",
  },
];

export default async function ConstitutionPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const p = (path: string) => (isZh ? `/zh${path}` : `/en${path}`);

  return (
    <>
      {/* THE THESIS */}
      <section className="mx-auto max-w-[1100px] px-6 pt-12 pb-10">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="法" />
          <div className="eyebrow">{isZh ? "立论" : "The thesis"}</div>
        </div>
        <h1 className="font-display tracking-crisp mb-6">
          {isZh ? "三层法" : "Three layers of law"}
          <span className="font-cjk text-indigo text-3xl sm:text-5xl ml-3">
            {isZh ? "Three layers of law" : "三层法"}
          </span>
        </h1>
        <p className={`max-w-2xl text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "项目一变老，指令就开始堆积、彼此冲突：最初的规格、后来推翻它的重构、陈旧的记忆、上一个智能体的交接、你此刻的要求、刚跑出的与交接说法不符的测试结果。扁平的系统提示词让模型靠猜来化解；CodeWhale 用一部嵌套的宪法给出明确的位阶。顺序由执行框架强制生效——有测试断言它不会漂移——换掉模型，结构依然完好。"
            : "As a project ages, instructions pile up and conflict: the original spec, a refactor that contradicts it, stale memory, a previous agent's handoff, your current request, fresh test output that doesn't match what the handoff claimed. A flat system prompt makes the model resolve that by guess. CodeWhale uses a nested constitution so there is a defined rank instead of vibes — the order is enforced in the harness, with tests asserting it can't drift, and it stays intact when you swap models."}
        </p>

        {/* flagship framing — version derived from generated facts, never hardcoded */}
        <div className="mt-7 max-w-2xl px-4 py-3 hairline-t hairline-b hairline-l hairline-r">
          <span className="pill pill-new mr-2">
            {isZh ? `v${FACTS.version ?? "0.8.x"} 新增` : `New in v${FACTS.version ?? "0.8.x"}`}
          </span>
          <span className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
            {isZh
              ? "宪法优先的初始设置——首次启动依次引导语言、模型、安全姿态和你的宪法；之后随时 /setup。模型可以起草，由你批准。"
              : "Constitution-first setup — first launch walks language, model, posture, and your constitution; /setup any time. The model can draft it. You ratify it."}
          </span>
        </div>
      </section>

      {/* THE THREE LAYERS */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-6">
          <Seal char="序" />
          <div className="eyebrow">{isZh ? "位阶，从最稳到最活" : "The rank, most-static first"}</div>
        </div>

        <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b">
          {LAYERS.map((layer) => (
            <div key={layer.n} className="p-6">
              <div className="font-mono uppercase tracking-widest mb-2 text-[0.7rem] text-indigo">{layer.n}</div>
              <h2 className="font-display text-xl mb-1">{isZh ? layer.name.zh : layer.name.en}</h2>
              <div className="font-mono text-[0.68rem] text-ink-mute mb-3 break-all">
                {isZh ? layer.pathZh : layer.path}
              </div>
              <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {isZh ? layer.zh : layer.en}
              </p>
            </div>
          ))}
        </div>

        <p className={`mt-5 max-w-2xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "三层之下依次是项目说明（AGENTS.md）、记忆与交接。你此刻的要求和实时工具证据仍然主宰当前回合——模型可以被给到很多层，但它绝不能报告一个工具没有返回的事实。"
            : "Below the three layers rank project instructions (AGENTS.md), then memory and handoffs. Your current request and live tool evidence still control the active turn — the model may be given many layers, but it may never report a fact the tools did not return."}
        </p>

        {/* The honest boundary */}
        <div className="mt-6 max-w-2xl px-4 py-3 hairline-t hairline-b hairline-l hairline-r text-sm">
          <div className="eyebrow mb-1.5">{isZh ? "诚实的边界" : "The honest boundary"}</div>
          <p className={`text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
            {isZh
              ? "审批、沙箱、网络与信任控制由代码强制执行——宪法文本永远越不过它们。"
              : "Approval, sandbox, network, and trust controls are enforced in code — constitution text never overrides them."}
          </p>
        </div>
      </section>

      {/* OBSERVABLE IN THE REASONING */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="证" />
          <div className="eyebrow">{isZh ? "在推理里可以被看到" : "Observable in the reasoning"}</div>
        </div>
        <p className={`mb-6 max-w-2xl text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "位阶不是落地页上的一句宣称。下面是真实会话的忠实片段——模型在裁决时直接援引条款。"
            : "The rank is not a claim on a landing page. These are faithful excerpts from a real session — the model cites the articles as it decides."}
        </p>
        <ThinkingTrace locale={locale} />
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
            href={p("/docs/constitution")}
            className="px-5 py-3 hairline-t hairline-b hairline-l hairline-r font-mono text-sm uppercase tracking-wider hover:bg-paper-deep transition-colors"
          >
            {isZh ? "参考细节：文档 →" : "Reference detail: docs →"}
          </Link>
          <Link
            href="https://github.com/Hmbown/CodeWhale/blob/main/docs/CONFIGURATION.md#constitution-project-instructions-and-repo-authority"
            className="px-5 py-3 font-mono text-sm uppercase tracking-wider text-ink-mute hover:text-indigo transition-colors"
          >
            {isZh ? "配置文档 ↗" : "Configuration ↗"}
          </Link>
        </div>
      </section>
    </>
  );
}
