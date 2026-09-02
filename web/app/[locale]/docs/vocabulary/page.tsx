import { StatusBadge } from "@/components/status-badge";
import {
  ADVISORY_ROLE,
  CONTROL_MODES,
  MEASUREMENT_PRINCIPLES,
  PERMISSION_POSTURES,
  PRODUCT_TERMS,
  ROUTE_IDENTITY,
} from "@/lib/content/vocabulary";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/vocabulary",
    locale,
    title: isZh ? "产品名词 · Codewhale 文档" : "Vocabulary · Codewhale Docs",
    description: isZh
      ? "确切的产品名词：Fleet、Workflow、Lane、Runtime、Advisor，Plan / Work / Operate 与权限姿态，以及请求→实际思考档位、路由来源与测量原则。"
      : "The exact product nouns: Fleet, Workflow, Lane, Runtime, Advisor, Plan / Work / Operate and permission postures, plus requested→effective reasoning, routing source, and measurement principles.",
  });
}

export default async function VocabularyPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{isZh ? "产品名词" : "Vocabulary"}</h1>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "这些名词在全站、TUI 和收据里含义完全一致。名词本身不翻译；每条定义都与仓库中的公共事实矩阵逐字对应。"
            : "These nouns mean the same thing on this site, in the TUI, and in receipts. The nouns themselves are never translated; every definition matches the public fact matrix in the repository verbatim."}
        </p>
      </section>

      <section id="execution-nouns" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "执行名词" : "Execution nouns"}
        </h2>
        <div className="hairline-t mt-4">
          {PRODUCT_TERMS.map((row) => (
            <section key={row.term} className="py-4 hairline-b">
              <h3 className="font-display text-xl">
                {row.term}{" "}
                <span className="text-sm text-ink-mute font-normal">
                  = {isZh ? row.short.zh : row.short.en}
                </span>
              </h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{isZh ? row.long.zh : row.long.en}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="control-nouns" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "模式与权限姿态" : "Modes and permission postures"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "模式（Tab 循环，输入框空闲时）决定可见的交互方式；权限姿态（Shift+Tab 循环）决定工具执行前询问的激进程度。两者正交。"
            : "Modes (Tab, composer idle) decide the visible interaction; permission postures (Shift+Tab) decide how aggressively tools ask before executing. The two are orthogonal."}
        </p>
        <div className="hairline-t mt-4">
          {[...CONTROL_MODES, ...PERMISSION_POSTURES].map((row) => (
            <section key={row.term} className="py-4 hairline-b">
              <h3 className="font-display text-xl">{row.term}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>
                {isZh ? row.description.zh : row.description.en}
              </p>
            </section>
          ))}
        </div>
      </section>

      <section id="route-identity" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh
            ? "路由身份：provider · 模型 · 请求→实际思考档位 · 来源"
            : "Route identity: provider · model · requested→effective reasoning · source"}
        </h2>
        <div className="hairline-t mt-4">
          {ROUTE_IDENTITY.map((row) => (
            <section key={row.term} className="py-4 hairline-b">
              <h3 className="font-display text-xl">{row.term}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>
                {isZh ? row.description.zh : row.description.en}
              </p>
            </section>
          ))}
        </div>
      </section>

      <section id="advisory-role" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "咨询角色" : "Advisory role"}
        </h2>
        <div className="hairline-t mt-4 py-4 hairline-b">
          <h3 className="font-display text-xl">{ADVISORY_ROLE.term}</h3>
          <p className={`${bodyClass} mt-1 text-sm`}>
            {isZh ? ADVISORY_ROLE.description.zh : ADVISORY_ROLE.description.en}
          </p>
        </div>
      </section>

      <section id="measurement" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "测量原则" : "Measurement principles"}
        </h2>
        <ul className="mt-4 space-y-3">
          {MEASUREMENT_PRINCIPLES.map((principle) => (
            <li key={principle.en} className={`${bodyClass} text-sm`}>
              {isZh ? principle.zh : principle.en}
            </li>
          ))}
        </ul>
        <p className={`${bodyClass} mt-4 text-sm`}>
          <StatusBadge kind="unavailable" locale={locale} />{" "}
          {isZh
            ? "基准排行榜：本站没有，也不会在没有路由身份与测量工具链的情况下出现。"
            : "Benchmark leaderboard: none on this site, and none will appear without route identity and a measurement harness attached."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/FLEET.md, docs/MODES.md, docs/public-surface-facts.json · 名词文案来自 web/lib/content/vocabulary.ts；更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/FLEET.md, docs/MODES.md, docs/public-surface-facts.json · Vocabulary copy lives in web/lib/content/vocabulary.ts; update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
