import type { DocsFleetDict } from "../types";

export const docsFleet: DocsFleetDict = {
  metaTitle: "Fleet 与 Workflow · Codewhale 文档",
  metaDescription: "持久 Agent 花名册与成员选择层，以及可选的 Workflow 编排层。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "Fleet 与 Workflow",
  overviewLead:
    "Fleet 是持久花名册：记录有哪些成员以及选中了哪一位。它不是执行或权限引擎。Runtime 将选定成员作为无头 codewhale exec 运行来启动和跟踪，负责重试与远程放置，并写入持久收据和台账投影。",
  runTitle: "运行一次 Fleet",
  runLead:
    "Runtime 的 Fleet 运行投影存放在工作区的 .codewhale/fleet.jsonl 台账中，worker 日志在 .codewhale/fleet/ 下。codewhale fleet resume <run-id> 会让 Runtime 重放台账并调和过期租约；该操作幂等，可在管理进程退出、笔记本睡眠或运行时重启后安全执行。",
  statusLead:
    "注意两个同名状态面：TUI 里的 {fleetStatusTui}（或 {subagents}）只显示当前交互会话的子 Agent；shell 里的 {fleetStatusShell} 才读取持久 Fleet 台账。",
  profilesTitle: "角色与 /fleet setup",
  profilesLead:
    "/fleet setup 打开渐进式向导来编写可复用的花名册成员：依次选择语义角色、模型（继承或具体已配置路由）和思考档位，再核对准确的身份与路由后保存。档案可写在项目级（.codewhale/agents/<role>.toml）或个人级（$CODEWHALE_HOME/agents/<role>.toml）；同 ID 的项目档案优先。Runtime 另行负责信任、文件系统/网络范围、密钥、审批、沙箱和工具，因此档案存储范围不会扩大执行权限。",
  workflowTitle: "Workflow 编排",
  workflowLead:
    "普通多 Agent 工作不需要 Workflow：在 Operate 里直接发消息，需要并行、隔离或长时间工作时让 Codewhale 优先委派后台 worker 即可。只有当工作需要有序阶段、门禁、共享预算、回放或确定性汇总时才用 Workflow。Workflow 脚本只负责协调：它选择 Fleet 成员，但没有自己的文件系统或 shell；Runtime 在实时权限策略下启动真正的 worker。脚本使用编译专用的声明式 JS 子集，降低到类型化 WorkflowSpec 后由 Rust 校验与执行；import、fetch、process、eval、async/await 会被拒绝。",
  workflowLimits:
    "默认校验边界：每次 Workflow 运行最多 1,000 个 worker Agent、最多 8 层递归 Fleet 环（默认 3 层）、循环必须声明 max_iterations、动态 expand 节点必须声明 max_children 和模板。这些是数量上限而非并发要求：Runtime 每个运行最多接纳 16 个存活 worker，其余排队。省略 max_steps 或设为 0 都保持无界；只有正值才增加模型轮次上限。",
  sourceNote: "来源文档：docs/FLEET.md, docs/WORKFLOW_AUTHORING.md · 更新时请同步修改 docs-map.ts。",
};
