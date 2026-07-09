import Link from "next/link";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/constitution",
    locale,
    title: isZh ? "宪法与 /constitution · CodeWhale 文档" : "Constitution and /constitution · CodeWhale Docs",
    description: isZh
      ? "用户全局宪法、仓库本地法、项目说明和运行时边界。"
      : "User-global constitution, repo-local law, project instructions, and runtime boundaries.",
  });
}

export default async function ConstitutionPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">
          {isZh ? "宪法与 /constitution" : "Constitution and /constitution"}{" "}
          <span className="font-cjk text-indigo text-2xl ml-2">
            {isZh ? "Constitution" : "宪法与 /constitution"}
          </span>
        </h2>
        {isZh ? (
          <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
            CodeWhale 先给 Agent 一个可追责的地址，再给上下文冲突一套法律。
            <code className="inline">/constitution</code> 是管理个人常驻宪法的主入口：
            它把结构化的用户全局设置保存在 <code className="inline">$CODEWHALE_HOME/constitution.json</code>，
            再渲染成模型可读的 prose block。仓库仍可通过{" "}
            <code className="inline">.codewhale/constitution.json</code> 增加本地 law；runtime
            policy 独立负责模式、审批、沙箱、成本和工具边界。
          </p>
        ) : (
          <p className="text-ink-soft mt-3 leading-relaxed">
            CodeWhale gives the agent an accountable address, then a legal system for
            context conflicts. <code className="inline">/constitution</code> is the
            primary personal constitution surface: guided setup stores structured
            user-global data in <code className="inline">$CODEWHALE_HOME/constitution.json</code>
            and renders it as model-facing prose. Repos can still add local law via{" "}
            <code className="inline">.codewhale/constitution.json</code>; runtime policy
            separately encodes modes, approval, sandbox, cost, and tool boundaries.
          </p>
        )}
        <div className="hairline-t hairline-b mt-6 grid md:grid-cols-3 col-rule">
          {[
            {
              name: "User-global",
              cn: "用户全局",
              en: "Use /constitution for standing personal law across projects. It is structured data rendered to prose, not a raw prompt editor.",
              zh: "用 /constitution 管理跨项目个人常驻法。它是结构化数据渲染成 prose，不是裸 prompt 编辑器。",
            },
            {
              name: "Repo-local",
              cn: "仓库本地",
              en: ".codewhale/constitution.json is optional project policy for protected invariants, branch rules, verification, and escalation.",
              zh: ".codewhale/constitution.json 是可选项目 law，用于不变量、分支规则、验证和升级条件。",
            },
            {
              name: "Runtime",
              cn: "运行时",
              en: "Constitution text may express preferences, but approval, sandbox, shell, network, trust, and MCP permissions remain enforced config.",
              zh: "宪法文本可以表达偏好；审批、沙箱、Shell、网络、信任和 MCP 权限仍由运行时配置强制执行。",
            },
          ].map((row) => (
            <div key={row.name} className="p-5">
              <div className="font-display text-lg text-indigo mb-1">
                {row.name} <span className="font-cjk text-sm ml-1.5">{row.cn}</span>
              </div>
              <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {isZh ? row.zh : row.en}
              </p>
            </div>
          ))}
        </div>
        <p className={`mt-4 text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "普通项目说明仍放在 AGENTS.md；记忆和交接低于宪法与项目说明；完整 base prompt Markdown 覆盖只是专家逃生口，不是普通设置路径。详见 "
            : "Standard project instructions still live in AGENTS.md; memory and handoffs rank below constitutions and project instructions; the full base-prompt Markdown override is an expert escape hatch, not the normal setup path. See "}
          <Link
            href="https://github.com/Hmbown/CodeWhale/blob/main/docs/CONFIGURATION.md#constitution-project-instructions-and-repo-authority"
            className="body-link"
          >
            {isZh ? "configuration docs" : "configuration docs"}
          </Link>
          {isZh ? "。" : "."}
        </p>
      </section>
      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/ARCHITECTURE.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/ARCHITECTURE.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
