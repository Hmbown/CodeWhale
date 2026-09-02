import type { DocsFleetDict } from "../types";

export const docsFleet: DocsFleetDict = {
  metaTitle: "Fleet 与 Workflow · Codewhale 文档",
  metaDescription: "持久 Agent 花名册与成员选择层，以及可选的 Workflow 编排层。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "Fleet 与 Workflow",
  overviewLead:
    "Fleet 是持久花名册：记录有哪些成员以及选中了哪一位。它不是执行或权限引擎。Runtime 将选定成员作为无头 codewhale exec 运行来启动和跟踪，负责重试与远程放置，并写入持久收据和台账投影。台账文件、已保存花名册、配置表和 Workflow 的 --fleet 标志都使用 Fleet 名称。",
  runTitle: "运行一次 fleet",
  runLead:
    "Runtime 的 fleet 运行投影存放在工作区的 .codewhale/fleet.jsonl 台账中，worker 日志在 .codewhale/fleet/ 下。codewhale fleet resume <run-id> 会让 Runtime 重放台账并调和过期租约；该操作幂等，可在管理进程退出、笔记本睡眠或运行时重启后安全执行。",
  statusLead:
    "TUI 命令 {fleetStatusTui} 与 shell 命令 {fleetStatusShell} 读取同一份持久 fleet 台账。若要查看仅附着于当前交互会话的子 Agent，请使用 {fleetWorkers}（或 {subagents}）。",
  profilesTitle: "已保存 fleet、角色与 /fleet setup",
  profilesLead:
    "{fleetSaved} 打开已命名保存 fleet 的选择器；裸 /fleet 打开当前所选 fleet 的成员花名册。/fleet setup 打开渐进式向导来编写可复用的花名册成员：依次选择语义角色、模型（继承或具体已配置路由）和思考档位，再核对准确的身份与路由后保存。档案可写在项目级（.codewhale/agents/<role>.toml）或个人级（$CODEWHALE_HOME/agents/<role>.toml）；同 ID 的项目档案优先。Runtime 另行负责信任、文件系统/网络范围、密钥、审批、沙箱和工具，因此档案存储范围不会扩大执行权限。",
  workflowTitle: "Workflow 编排",
  workflowLead:
    "普通多 Agent 工作不需要 Workflow：在 Operate 里直接发消息，需要并行、隔离或长时间工作时让 Codewhale 优先委派后台 worker 即可。只有当工作需要有序阶段、门禁、共享预算、回放或确定性汇总时才用 Workflow。Workflow 脚本只负责协调：它选择 fleet 成员，但没有自己的文件系统或 shell；Runtime 在实时权限策略下启动真正的 worker。脚本使用编译专用的声明式 JS 子集，降低到类型化 WorkflowSpec 后由 Rust 校验与执行；import、fetch、process、eval、async/await 会被拒绝。",
  workflowLimits:
    "默认校验边界：每次 Workflow 运行最多 1,000 个 worker Agent、Workflow IR 结构嵌套深度不超过 5、循环必须声明 max_iterations、动态 expand 节点必须声明 max_children 和模板。Runtime 的子委派是独立执行预算：默认 3 层，可选择启用的硬上限为 8 层。这些是数量与结构上限，而非并发要求：Runtime 每个运行最多接纳 16 个存活 worker，其余排队。省略 max_steps 或设为 0 都保持无界；只有正值才增加模型轮次上限。",
  sourceNote: "来源文档：docs/FLEET.md, docs/WORKFLOW_AUTHORING.md · 更新时请同步修改 docs-map.ts。",
};
