import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/modes",
    locale,
    title: isZh ? "模式 · Codewhale 文档" : "Modes · Codewhale Docs",
    description: isZh
      ? "Plan、Work、Operate 三种运行模式与独立的权限姿态。"
      : "Plan, Work, Operate modes and independent permission postures.",
  });
}

export default async function ModesPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const modes = isZh
    ? [
        {
          name: "Plan",
          description:
            "用于只读调查与规划。Codewhale 可以检查工作区，但不能执行 Shell 命令或修改文件。",
        },
        {
          name: "Act",
          description:
            "用于常规交互式编码。Codewhale 可以检查、编辑并使用工具；Shell 是否可用以及何时请求批准，取决于当前配置和权限姿态。",
        },
        {
          name: "Operate",
          description:
            "用于从同一个输入区协调多项任务。父回合可以直接检查、编辑并使用 Shell 或 MCP 工具，其权限姿态、沙箱和安全规则与 Act 相同。独立、并行、后台或长时间工作会优先交给 Fleet worker，但并非所有可执行步骤都必须委派。只有需要有序阶段、门禁或确定性汇总时才需要 Workflow。",
        },
      ]
    : [
        {
          name: "Plan",
          description:
            "Read-only investigation and planning. Codewhale can inspect the workspace, but it cannot run shell commands or edit files.",
        },
        {
          name: "Act",
          description:
            "Normal interactive coding. Codewhale can inspect, edit, and use tools; shell availability and approval prompts follow the active configuration and permission posture.",
        },
        {
          name: "Operate",
          description:
            "Multitask coordination from the same composer. The parent can inspect, edit, and use shell or MCP tools under the same permission posture, sandbox, and safety rules as Act. Fleet workers are preferred for independent, parallel, background, or long-running work, but delegation is not required for every executable step. Workflow is optional unless the work needs ordered phases, gates, or deterministic fan-in.",
        },
      ];
  const postures = isZh
    ? [
        {
          name: "Ask",
          description: "在可能产生重要后果的工具执行前询问你。",
        },
        {
          name: "Auto-Review",
          description: "自动评估工具风险，只在确实需要你决定时询问。",
        },
        {
          name: "Full Access",
          description:
            "无需批准提示即可运行工具，并启用受信任工作区访问。仓库规则和托管约束仍然有效；仅在你信任的工作区中使用。",
        },
      ]
    : [
        {
          name: "Ask",
          description: "Ask before tools that can make consequential changes.",
        },
        {
          name: "Auto-Review",
          description: "Review tool risk automatically and ask when a decision needs you.",
        },
        {
          name: "Full Access",
          description:
            "Run tools without approval prompts and enable trusted-workspace access. Repository rules and managed constraints still apply; use it only in a workspace you trust.",
        },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "模式" : "Modes"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "模式决定 Codewhale 如何组织工作；权限姿态决定它如何处理具有后果的工具调用。两者相互独立。"
            : "A mode decides how Codewhale handles the work. A permission posture decides how it handles consequential tool calls. They are separate controls."}
        </p>
        <div className="hairline-t mt-6">
          {modes.map((mode) => (
            <section key={mode.name} className="py-4 hairline-b">
              <h3 className="font-display text-xl">{mode.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{mode.description}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="switching" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "切换模式" : "Switch modes"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              输入区空闲时，按 <kbd className="font-mono text-xs px-1.5 py-0.5 hairline">Tab</kbd>{" "}
              循环 Plan → Act → Operate。补全菜单打开时，Tab 接受补全；回合运行时，它可以把当前草稿排入下一个跟进消息。
            </>
          ) : (
            <>
              When the composer is idle, press{" "}
              <kbd className="font-mono text-xs px-1.5 py-0.5 hairline">Tab</kbd> to cycle Plan →
              Act → Operate. When a completion menu is open, Tab accepts the completion; during
              an active turn, it can queue the current draft as the next follow-up.
            </>
          )}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "运行 /mode 打开模式选择器，或使用以下命令直接切换："
            : "Run /mode to open the picker, or switch directly:"}
        </p>
        <pre className="code-block mt-4">{`/mode plan
/mode act
/mode operate`}</pre>
      </section>

      <section id="permissions" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "权限姿态" : "Permission postures"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              Plan 始终为只读。在 Act 或 Operate 中且输入区空闲时，按{" "}
              <kbd className="font-mono text-xs px-1.5 py-0.5 hairline">Shift+Tab</kbd> 循环 Ask
              → Auto-Review → Full Access。运行 <code className="inline">/config</code>{" "}
              可查看或编辑当前会话权限；项目或托管策略可能会锁定或收紧它。
            </>
          ) : (
            <>
              Plan is always Read Only. When the composer is idle in Act or Operate, press{" "}
              <kbd className="font-mono text-xs px-1.5 py-0.5 hairline">Shift+Tab</kbd> to cycle Ask
              → Auto-Review → Full Access. Run <code className="inline">/config</code> to inspect
              or edit the current session permission; project or managed policy may lock or
              tighten it.
            </>
          )}
        </p>
        <div className="hairline-t mt-6">
          {postures.map((posture) => (
            <section key={posture.name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{posture.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{posture.description}</p>
            </section>
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
