import type { DocsComputersDict } from "../types";

/**
 * Simplified-Chinese dictionary for `app/[locale]/docs/computers/page.tsx`.
 * Statements trace to docs/DAYTONA_CLOUD_DISPATCH.md and docs/CODEWHALE_AGENT.md.
 */
export const docsComputers: DocsComputersDict = {
  metaTitle: "云端计算机 · Codewhale 文档",
  metaDescription:
    "本地 Codewhale 会话如何在 Daytona 计算机上提议、确认并跟踪一个云端 Agent——显式指定代码托管平台、凭证缺失即拒绝，以及哪些部分尚未实现。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "云端计算机",
  overviewLead:
    "本地会话可以把一项编码任务卸载到云端计算机，就像编辑器派出一个云端 Agent：远程任务创建分支，并计划向明确指定的代码托管平台发起 pull request。本地保持响应。花费与推送不会悄悄发生——先写入提议，在你确认之前不会创建沙箱，也不会推送分支。",
  proposeTitle: "先提议，再确认",
  proposeLead:
    "第一条命令写入提议并退出。第二条按 id 确认。TUI 中有同样的两步斜杠命令，{cloudAgent} 是 {dispatch} 的别名。",
  jobsLead: "云端任务作为 {kind} 在现有的任务面板上是一等公民。可以在 TUI 或 CLI 中列出、查看和取消：",
  remotesTitle: "显式指定托管平台",
  remotesLead:
    "远程仓库不会被默认当作 GitHub。以平台命名的远程就是该平台；其他远程按 URL 主机判定。如果存在多个平台，请传入 {remoteFlag}。",
  remotes: [
    ["github · cnb · gitee（按远程名）", "对应平台，与 URL 无关"],
    ["origin 或其他 → github.com", "github"],
    ["origin 或其他 → cnb.cool", "cnb"],
    ["origin 或其他 → gitee.com", "gitee"],
  ],
  enableTitle: "启用 Daytona 计算机",
  enableLead: "凭证只存在于进程环境或 Codewhale 密钥存储中——绝不写进 config.toml 或 models.toml，也绝不提交到仓库。",
  enableSteps: [
    ["创建 API 密钥", "在 Daytona 控制台的 API keys 页面。"],
    ["为本次会话导出", "{apiKey}；如需非默认端点，可选设置 {apiUrl}。"],
    ["或一次性保存", "存入 Codewhale 密钥槽 {slot}（OS 钥匙串或 $CODEWHALE_HOME 密钥文件）。也接受 {alias} 别名。"],
  ],
  cliNote: "安装了 daytona CLI 并不等于有凭证。{status} 和不带参数的 {bare} 会分别报告 CLI 是否存在与凭证是否存在。",
  rulesTitle: "缺失即拒绝的规则",
  rules: [
    ["未确认", "写入 {proposed} 任务；命令成功退出；不调用 Daytona；不推送任何内容。"],
    ["已确认但无凭证", "写入 {refused} 任务；命令失败退出；不存在沙箱。"],
    ["已确认且有凭证", "创建 Daytona 沙箱，并以任务 id 和平台打标签。此版本不会声称任何 GitHub、CNB 或 Gitee 的 PR 地址；缺少平台令牌时同样拒绝。"],
  ],
  membershipTitle: "谁可以派发",
  membershipLead:
    "托管 Agent 界面使用同一个 Codewhale 会员身份认证——即 {login} 的账户会话。会员资格决定云端 Agent，而不是本地派发：只要有 Daytona 与托管平台凭证，`codewhale dispatch` 无需账户即可运行。提供商品牌保持内部不可见，安装和运行本地 Runtime 完全不需要账户。",
  leftoverTitle: "尚未实现",
  leftover: [
    ["实时查看", "正在运行的沙箱的日志跟随。"],
    ["真正的取消", "从任务面板销毁付费的 Daytona 沙箱。"],
    ["自动决定", "Codewhale 可以提议派发，但不能自己确认自己的提议。"],
    ["远程执行器", "真正创建分支并发起 pull request 的 Agent。"],
  ],
  sourceNote: "源文档：docs/DAYTONA_CLOUD_DISPATCH.md、docs/CODEWHALE_AGENT.md · 修改时请同步更新 docs-map.ts。",
};
