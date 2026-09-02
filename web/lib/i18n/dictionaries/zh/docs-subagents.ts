import type { DocsSubagentsDict } from "../types";

/** 中文对照见 `en/docs-subagents.ts`,文案自页面的 `isZh` 三元逐字迁入。 */
export const docsSubagents: DocsSubagentsDict = {
  metaTitle: "子 Agent · Codewhale 文档",
  metaDescription: "agent 工具、fleet 角色、上下文分叉、worktree 隔离和并发上限。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "子 Agent",
  overviewLead:
    "父会话通过 agent 工具启动一个有明确职责的子 Agent，并立即拿回 agent_id、compact 收据和 transcript 句柄；子 Agent 在后台运行。子 Agent 默认继承父级的工具注册表，但它们是叶子 worker：不会再拿到 agent 或嵌套生命周期工具。agent 启动的是分离的后台工作——取消父回合会停止父级的等待路径，但不会杀死已经启动的子运行。",
  overviewFleetNote:
    "对于必须跨进程重启、睡眠或远程执行存活的工作，优先选择 fleet 或 Workflow 支撑的 fleet 运行，而不是会话内的短寿命 agent 调用。",
  roles: [
    ["worker", "灵活执行父级交代的多步任务；可写、可用 shell。默认角色。"],
    ["scout", "只读，快速摸清相关代码——例如“找出 Foo 的所有调用点”。"],
    ["planner", "分析并产出策略，不执行——“设计迁移方案，不要动手”。"],
    ["reviewer", "只读审查并按严重度打分——“审一遍这个 PR 的 bug”。"],
    ["builder", "以最小改动落地一个明确的变更；可写、可用 shell。"],
    ["verifier", "运行测试和校验并汇报结果，不写代码。"],
    ["consultant", "只读的高推理力度顾问，用于判断类问题和设计评审。"],
    ["custom", "手工指定狭窄的工具白名单，用于锁定的派发。"],
  ],
  forkTitle: "上下文分叉",
  forkLead:
    "{agentTool} 默认开启全新会话：子 Agent 只拿到角色提示词和你给的任务。当任务依赖父 transcript 里已有的决定、文件、待办或计划状态时，用 {forkContext}——运行时在可用时保持父级前缀逐字节一致（保留前缀缓存复用），追加一份结构化状态快照，再把子 Agent 的角色说明和任务放在末尾。独立探索用新会话，延续、审查、总结或压缩类工作用分叉会话。",
  worktreeTitle: "Worktree 隔离",
  // No space before 值: the old JSX put a line break after the
  // `coordination_contracts` span, and JSX drops leading whitespace that
  // carries a newline. Reproduced so this stays a move, not a copy edit.
  worktreeLead:
    "并行编辑通道用 {worktreeFlag} 启动：Codewhale 为子 Agent 创建新的 git worktree 和分支（默认 {branchPattern}，检出在父仓库旁的 {worktreeDir} 下），父检出保持干净。隔离不等于写权限：只带 prompt 的 worker 从只读开始；要写代码的子 Agent 还需声明 {writeAuthority} 和至少一个规范化的 {writeRoots}、{exactFiles} 或 {coordinationContracts}值；重叠的共享写声明会在任何改动之前失败。",
  capacityTitle: "并发上限",
  capacityLead:
    "子 Agent 容量的权威来源是 crates/tui/src/config/subagent_limits.rs：默认配置并发 64，最大配置并发 128，运行加排队的最大准入 1024。这些是容量上限，不是建议把每个槽位都派出去——管理者应使用最小的有效扇出，保持单一汇总负责人，并在汇报整体完成前验证 worker 收据。",
  sourceNote: "来源文档：docs/SUBAGENTS.md · 更新时请同步修改 docs-map.ts。",
};
