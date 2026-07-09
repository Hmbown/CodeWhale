import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/modes",
    locale,
    title: isZh ? "模式 · CodeWhale 文档" : "Modes · CodeWhale Docs",
    description: isZh
      ? "Plan、Agent、YOLO 三种运行模式与正交审批策略。"
      : "Plan, Agent, YOLO operating modes and orthogonal approval policies.",
  });
}

export default async function ModesPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">
          {isZh ? "模式" : "Modes"}{" "}
          <span className="font-cjk text-indigo text-2xl ml-2">
            {isZh ? "Modes" : "模式"}
          </span>
        </h2>
        {isZh ? (
          <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
            三种运行模式——与审批系统正交。按{" "}
            <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">
              Tab
            </kbd>{" "}
            切换。
          </p>
        ) : (
          <p className="text-ink-soft mt-3 leading-relaxed">
            Three operating modes — orthogonal to the approval system. Press{" "}
            <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">
              Tab
            </kbd>{" "}
            to cycle.
          </p>
        )}
        <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b mt-6">
          {[
            {
              name: "Plan",
              cn: "计划",
              color: "text-cobalt",
              en: "Read-only investigation. Grep, read files, list directories, fetch URLs — cannot write or execute shell.",
              zh: "只读调查。可以 grep、读文件、列目录、抓取 URL——不能写入或执行 shell。",
            },
            {
              name: "Agent",
              cn: "代理",
              color: "text-jade",
              en: "Default mode. Multi-step tool use. Shell and side-effect tools require approval per approval_mode.",
              zh: "默认模式。多步工具调用。Shell 和有副作用的工具需按 approval_mode 设置审批。",
            },
            {
              name: "YOLO",
              cn: "全权",
              color: "text-indigo",
              en: "Auto-approve all actions and enable trust mode. Workspace boundary lifted. Use carefully.",
              zh: "自动批准所有操作并启用信任模式。工作区边界解除。请谨慎使用。",
            },
          ].map((m) => (
            <div key={m.name} className="p-5">
              <div className={`font-display text-xl ${m.color} mb-1`}>
                {m.name} <span className="font-cjk text-base ml-1.5">{m.cn}</span>
              </div>
              <p className={`text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {isZh ? m.zh : m.en}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section id="approval" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "审批策略" : "Approval Policies"}
        </h2>
        {isZh ? (
          <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
            模式与审批是两个独立的维度。通过 <code className="inline">/config</code> 设置。
          </p>
        ) : (
          <p className="text-ink-soft mt-3 leading-relaxed">
            Modes and approval are independent dimensions. Set via{" "}
            <code className="inline">/config</code>.
          </p>
        )}
        <div className="hairline-t hairline-b mt-6 grid md:grid-cols-3 col-rule">
          {[
            {
              name: "suggest",
              cn: "建议",
              en: "Default — follow mode rules. Ask before dangerous operations.",
              zh: "默认——按模式规则执行。危险操作前询问。",
            },
            {
              name: "auto",
              cn: "自动",
              en: "Auto-approve all tool calls. Equivalent to YOLO without trust.",
              zh: "自动批准所有工具调用。等同于无信任的 YOLO。",
            },
            {
              name: "never",
              cn: "拒绝",
              en: "Block any non-safe / non-read-only action. Investigation only.",
              zh: "阻止任何非安全/非只读操作。仅限调查。",
            },
          ].map((a) => (
            <div key={a.name} className="p-5">
              <div className="font-mono text-sm text-indigo uppercase tracking-wider">
                {a.name} ·{" "}
                <span className="font-cjk normal-case tracking-normal">{a.cn}</span>
              </div>
              <p className={`text-sm text-ink-soft mt-2 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {isZh ? a.zh : a.en}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/MODES.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/MODES.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
