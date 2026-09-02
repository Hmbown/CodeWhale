import type { DocsModesDict } from "../types";

export const docsModes: DocsModesDict = {
  metaTitle: "模式 · Codewhale 文档",
  metaDescription: "Plan、Work、Operate 三种运行模式与独立的权限姿态。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "模式",
  overviewLead: "模式决定 Codewhale 如何组织工作；权限姿态决定它如何处理具有后果的工具调用。两者相互独立。",
  modes: [
    ["Plan", "用于只读调查与规划。Codewhale 可以检查工作区，但不能执行 Shell 命令或修改文件。"],
    ["Act", "用于常规交互式编码。Codewhale 可以检查、编辑并使用工具；Shell 是否可用以及何时请求批准，取决于当前配置和权限姿态。"],
    ["Operate", "用于从同一个输入区协调多项任务。父回合可以直接检查、编辑并使用 Shell 或 MCP 工具，其权限姿态、沙箱和安全规则与 Act 相同。独立、并行、后台或长时间工作会优先交给 fleet worker，但并非所有可执行步骤都必须委派。只有需要有序阶段、门禁或确定性汇总时才需要 Workflow。"],
  ],
  switchingTitle: "切换模式",
  switchingLead:
    "输入区空闲时，按 {tab} 循环 Plan → Act → Operate。补全菜单打开时，Tab 接受补全；回合运行时，它可以把当前草稿排入下一个跟进消息。",
  switchingCommandLead: "运行 /mode 打开模式选择器，或使用以下命令直接切换：",
  permissionsTitle: "权限姿态",
  permissionsLead:
    "Plan 始终为只读。在 Act 或 Operate 中且输入区空闲时，按 {shiftTab} 循环 Ask → Auto-Review → Full Access。运行 {configCommand} 可查看或编辑当前会话权限；项目或托管策略可能会锁定或收紧它。",
  postures: [
    ["Ask", "在可能产生重要后果的工具执行前询问你。"],
    ["Auto-Review", "自动评估工具风险，只在确实需要你决定时询问。"],
    ["Full Access", "无需批准提示即可运行工具，并启用受信任工作区访问。仓库规则和托管约束仍然有效；仅在你信任的工作区中使用。"],
  ],
  sourceNote: "来源文档：docs/MODES.md · 更新时请同步修改 docs-map.ts。",
};
