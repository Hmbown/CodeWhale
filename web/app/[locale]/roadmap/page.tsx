import Link from "next/link";
import { getCachedRoadmap, type RoadmapItem } from "@/lib/roadmap-feed";
import { getEnv } from "@/lib/kv";
import { buildPageMetadata } from "@/lib/page-meta";

export const revalidate = 1800;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/roadmap",
    locale,
    title: isZh ? "路线图 · Codewhale" : "Roadmap · Codewhale",
    description: isZh
      ? "Codewhale 已完成、进行中、考虑中和明确不在范围内的工作。"
      : "Current Codewhale work grouped by shipped, underway, considered, and deliberately out-of-scope directions.",
  });
}

const tracksEn = [
  {
    title: "Shipped",
    items: [
      { title: "Small searchable toolbox", note: "Every turn starts with read, write, edit, bash, agent, and tool_search. Specialized native, Web, MCP, plugin, memory, task, and verification tools are policy-filtered and searchable; activated schemas stay in a bounded per-conversation toolbox." },
      { title: "Sub-agent parallel execution", note: "agent; 64 concurrent sessions by default, configurable to 128, with bounded result handles" },
      { title: "RLM batched processing", note: "Persistent sandboxed Python REPL with 1–16 cheap parallel children for long-input analysis" },
      { title: "Three operating modes", note: "Plan (read-only authority), Work (execution), and Operate (Fleet/Workflow orchestration) keep one primitive vocabulary; Ask / Auto-Review / Full Access remains an orthogonal approval posture." },
      { title: "OS command sandbox", note: "Seatbelt on macOS when available; opt-in bubblewrap on Linux when installed. Windows currently reports no OS sandbox." },
      { title: "Durable sessions + tasks", note: "Save, resume, rollback; background task queue with replayable timelines under ~/.codewhale/tasks/" },
      { title: "Bidirectional MCP", note: "Consume tools from external servers; expose as server via `codewhale mcp`; ~/.codewhale/mcp.json" },
      { title: "Skills + unified slash palette", note: "~/.codewhale/skills/ auto-loading; /help, /mode, /status, /config, /trust, /feedback" },
      { title: "OpenRouter provider", note: "OpenRouter integration with 300+ models across dozens of providers" },
      { title: "OpenAI-compatible & local runtimes", note: "Generic `openai` route for any OpenAI-compatible gateway, plus vLLM, SGLang, and Ollama against your own localhost endpoints — no key required" },
      { title: "Multi-provider support", note: "Hot-swap between providers (DeepSeek, OpenAI, Anthropic, OpenRouter) per session" },
      { title: "Local web client", note: "Implemented in the v0.9.1 source candidate: `codewhale web` is a loopback-only browser client over the Runtime API behind a one-time bootstrap session boundary; approvals and user input recover across page reloads (#4423)" },
      { title: "Anonymous usage counting", note: "On by default with a clear first-run disclosure and a durable opt-out. It posts aggregate session, feature, and error counts and closed enums to the first-party endpoint https://telemetry.codewhale.net/v1/telemetry, a Cloudflare Worker whose full source is in the repo under telemetry-ingest/. Its storage has no IP, country, or geo column — that is structural, not a setting — nothing is logged, and Cloudflare's retention is a fixed three months. Set telemetry_endpoint = \"\" to contact nobody: batches then go to $CODEWHALE_HOME/telemetry/dryrun.jsonl and you can read exactly what would have been sent. Never conversations, code, prompts, files, file/repo/branch names, model content, credentials, or a per-turn/per-tool timeline; the full schema is docs/TELEMETRY.md. Turn it off with `codewhale config set telemetry false` or CODEWHALE_TELEMETRY=0." },
    ],
  },
  {
    title: "Underway",
    items: [
      { title: "VS Code extension", note: "The repository ships a Phase 0 local-runtime companion: terminal launch, health checks, read-only thread summaries, and restore-point browsing. Full chat and editor actions are not part of this slice." },
      { title: "Memory typed store", note: "The native store has shipped: Markdown is the durable source of truth, with a rebuildable SQLite FTS5 index over it. Graph-structured agent memory and multi-signal recall are still ahead (#534–#536)" },
      { title: "Feishu / Lark bot", note: "First long-connection bridge over the runtime API shipped; richer chat features underway (#757)" },
      { title: "Chinese-market & i18n", note: "Locale-aware UI, platform refinements, region-specific search backends (#755)" },
      { title: "Hugging Face model discovery + Model Lab", note: "Browse, download, and manage models from Hugging Face Hub directly in the TUI" },
    ],
  },
  {
    title: "Considered",
    items: [
      { title: "Workrooms", note: "Durable, addressable agent-work threads over the Runtime API and user surfaces (#3209, docs/WORKROOM_ARCHITECTURE.md)" },
      { title: "Exa web-search backend", note: "Bundled alternative to the existing DDG + Bing path (#431)" },
      { title: "Homebrew core formula", note: "Tap exists; pursuing homebrew-core inclusion" },
      { title: "Native Windows installer", note: "MSI / WinGet; Scoop manifest already ships" },
      { title: "Unsloth / NeMo / Arcee fine-tune integration", note: "One-click fine-tuning workflows backed by Unsloth, NVIDIA NeMo, and Arcee toolkits" },
    ],
  },
  {
    title: "Ruled out",
    items: [
      { title: "Silent or content-level telemetry", note: "Anonymous usage counting is on by default, but the first-run disclosure and every opt-out are mandatory. What stays ruled out: conversations, code, prompts, files, model content, credentials, per-turn or per-tool timelines, collection hidden from the user, and any third-party ad or analytics SDK in the runtime binary. A selected hosted provider still receives the context required for its model turn; loopback routes can keep inference local." },
      { title: "Mandatory hosted relay for local sessions", note: "The local runtime and bring-your-own-provider routes continue to work without sending sessions through a Codewhale service" },
      { title: "Required account for the local runtime", note: "Installing and running Codewhale locally requires no account" },
      { title: "Sponsored model promotion", note: "Model picker stays neutral — no paid placement" },
      { title: "Public share links for local sessions", note: "The retired share-link direction (#471/#481) is not coming back; any future sharing design starts fresh." },
    ],
  },
  {
    title: "Open model platform",
    items: [
      { title: "Community model registry", note: "Discover, share, and rate community fine-tuned models with reproducible recipes" },
      { title: "One-click deploy", note: "Deploy any model to RunPod, Replicate, or your own infra with a single command" },
      { title: "Model evaluation dashboard", note: "Transparent, reproducible comparisons across providers, quantization levels, and hardware" },
    ],
  },
];

const tracksZh = [
  {
    title: "已完成",
    items: [
      { title: "小型可搜索工具箱", note: "每轮以 read、write、edit、bash、agent 与 tool_search 开始。原生专用工具、Web、MCP、插件、记忆、任务和验证工具按策略过滤并可搜索；激活的 schema 保存在有界的会话工具箱中。" },
      { title: "子 Agent 并行执行", note: "agent；默认 64 个并发会话，可配置到 128 个，通过 var_handle 有界读取结果" },
      { title: "RLM 批量处理", note: "持久沙箱 Python REPL，支持 1–16 路廉价并行子调用，处理长文本分析" },
      { title: "三种运行模式", note: "Plan（只读权限）、Work（执行）与 Operate（Fleet / Workflow 编排）使用同一套基础工具名称；Ask、Auto-Review 与 Full Access 审批姿态独立设置。" },
      { title: "OS 命令沙箱", note: "macOS 在可用时使用 Seatbelt；Linux 在安装后可显式启用 bubblewrap。Windows 当前报告无 OS 沙箱。" },
      { title: "持久化会话 + 后台任务", note: "保存、恢复、回滚；后台任务队列，可回放时间线，位于 ~/.codewhale/tasks/" },
      { title: "双向 MCP 协议", note: "消费外部服务器工具；通过 `codewhale mcp` 暴露为服务器；~/.codewhale/mcp.json" },
      { title: "技能 + 统一命令面板", note: "~/.codewhale/skills/ 自动加载；/help、/mode、/status、/config、/trust、/feedback" },
      { title: "OpenRouter 提供商", note: "原生集成 OpenRouter，支持 300+ 模型，覆盖数十个提供商" },
      { title: "OpenAI 兼容与本地运行时", note: "通用 `openai` 路由可接入任意 OpenAI 兼容网关；vLLM、SGLang、Ollama 直连本地端点，无需密钥" },
      { title: "多提供商支持", note: "按会话动态切换提供商（DeepSeek、OpenAI、Anthropic、OpenRouter）" },
      { title: "本地 Web 客户端", note: "v0.9.1 源码候选版已实现：`codewhale web` 是基于 Runtime API 与一次性引导会话边界的回环地址浏览器客户端；审批与用户输入可在页面刷新后恢复（#4423）" },
      { title: "匿名使用计数", note: "默认开启，首次运行会清楚说明，并提供永久关闭选项。它只会把聚合的会话、功能与错误计数以及封闭枚举 POST 到第一方端点 https://telemetry.codewhale.net/v1/telemetry；完整源码在 telemetry-ingest/。存储中没有 IP、国家或地理位置列，不写日志，Cloudflare 的保留期固定为三个月。设置 telemetry_endpoint = \"\" 可完全不联系服务器，批次只写入 $CODEWHALE_HOME/telemetry/dryrun.jsonl。永远不收集对话、代码、prompt、文件、文件/仓库/分支名、模型内容、凭据，也不发送逐轮或逐工具时间线；完整 schema 见 docs/TELEMETRY.md。用 `codewhale config set telemetry false` 或 CODEWHALE_TELEMETRY=0 关闭。" },
    ],
  },
  {
    title: "进行中",
    items: [
      { title: "VS Code 扩展", note: "仓库已提供 Phase 0 本地 Runtime 配套扩展：终端启动、健康检查、只读线程摘要和还原点浏览；完整聊天与编辑器操作尚未包含在此版本中。" },
      { title: "记忆类型化存储", note: "原生存储已发布：以 Markdown 作为持久化事实来源，并在其上建立可重建的 SQLite FTS5 索引。图结构 Agent 记忆与多信号召回仍在规划中（#534–#536）" },
      { title: "飞书 / Lark 机器人", note: "基于 runtime API 的长连接桥接已发布首版；更丰富的对话能力进行中（#757）" },
      { title: "中国市场与国际化改进", note: "本地化 UI、平台优化、区域搜索引擎（#755）" },
      { title: "Hugging Face 模型发现 + 模型实验室", note: "在 TUI 中直接浏览、下载和管理 Hugging Face Hub 上的模型" },
    ],
  },
  {
    title: "考虑中",
    items: [
      { title: "Workrooms 工作间", note: "基于 Runtime API 与用户界面的持久、可寻址 Agent 工作线程（#3209，docs/WORKROOM_ARCHITECTURE.md）" },
      { title: "Exa 网页搜索后端", note: "内建替代 DDG + Bing 的搜索路由（#431）" },
      { title: "Homebrew 核心仓库", note: "Tap 已有；正在争取进入 homebrew-core" },
      { title: "Windows 原生安装器", note: "MSI / WinGet；Scoop 清单已发布" },
      { title: "Unsloth / NeMo / Arcee 微调集成", note: "一键微调工作流，由 Unsloth、NVIDIA NeMo 和 Arcee 工具链驱动" },
    ],
  },
  {
    title: "暂不考虑",
    items: [
      { title: "静默或内容级遥测", note: "匿名使用计数默认开启，但首次运行说明与所有关闭选项都是强制的。仍被排除在外的是：对话、代码、prompt、文件、模型内容、凭据、逐轮或逐工具时间线、对用户隐藏的收集，以及运行时二进制中的任何第三方广告或分析 SDK。选用托管 provider 时仍会发送本轮所需上下文，回环地址路由可让推理保持本地。" },
      { title: "本地会话强制经过托管中继", note: "本地 Runtime 与自带提供商路由继续工作，无需把会话发送到 Codewhale 服务" },
      { title: "本地 Runtime 强制注册账户", note: "本地安装和运行 Codewhale 不需要账户" },
      { title: "赞助商模型推广", note: "模型选择器保持中立——无付费推荐位" },
      { title: "本地会话的公开分享链接", note: "已停用的分享链接方向（#471/#481）不会恢复；未来的分享设计将重新开始。" },
    ],
  },
  {
    title: "开放模型平台",
    items: [
      { title: "社区模型注册中心", note: "发现、分享和评价社区微调模型，附带可复现的配方" },
      { title: "一键部署", note: "一条命令将任意模型部署到 RunPod、Replicate 或自有基础设施" },
      { title: "模型评测面板", note: "跨提供商、量化级别和硬件的透明、可复现对比" },
    ],
  },
];

const roadmapText = (text: string) =>
  text.replace(/^>\s*/, "").replaceAll("**", "").replaceAll("CodeWhale", "Codewhale");

export default async function RoadmapPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const baseTracks = isZh ? tracksZh : tracksEn;

  // Live feed: shipped from GitHub Releases; underway/considered/ruled-out from issue labels.
  // Per-category fallback to the static items so unlabeled categories stay populated.
  let tracks = baseTracks;
  try {
    const env = await getEnv();
    const feed = await getCachedRoadmap(env.CURATED_KV, env.GITHUB_TOKEN);
    if (feed) {
      const liveByCategory: Record<string, RoadmapItem[]> = {
        Shipped: feed.shipped,
        Underway: feed.underway,
        Considered: feed.considered,
        "Ruled out": feed.ruledOut,
        已完成: feed.shipped,
        进行中: feed.underway,
        考虑中: feed.considered,
        暂不考虑: feed.ruledOut,
      };
      tracks = baseTracks.map((t) => {
        const live = liveByCategory[t.title];
        if (live && live.length > 0) {
          return { ...t, items: live.map((it) => ({ title: it.title, note: it.note })) };
        }
        return t;
      });
    }
  } catch {
    /* keep static fallback */
  }

  const copy = isZh
    ? {
        eyebrow: "项目路线图",
        title: "路线图",
        introduction: "这里将已完成的仓库工作、正在推进的工作、仍在评估的方案和明确不在范围内的方向分开列出。路线图的“已完成”可包含已在源码候选版中实现的工作；安装页与首页另行标明最新已发布包。发布记录和 GitHub issues 会在可用时更新这些分类。",
        sectionLabel: "当前状态",
        sectionTitle: "按状态查看工作",
        browseIssues: "浏览 open issues ↗",
        count: (value: number) => `${value} 项`,
        contributeLabel: "参与贡献",
        contributeTitle: "路线图决策公开进行。",
        contributeBody: "Bug 和范围明确的功能请求请使用 issues；尚在形成中的想法可以先在 Discussions 讨论；已有具体实现时，欢迎发送带测试或文档的 pull request。来自不同语言、平台和提供商的验证结果都能帮助维护者判断优先级。",
        links: [
          { title: "Issues", detail: "报告问题，或提出范围明确的工作。", href: "https://github.com/Hmbown/CodeWhale/issues" },
          { title: "Discussions", detail: "在开始实现前讨论尚未成熟的想法。", href: "https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas" },
          { title: "Pull requests", detail: "审查现有改动，或发送一个范围清楚的补丁。", href: "https://github.com/Hmbown/CodeWhale/pulls" },
        ],
      }
    : {
        eyebrow: "Project roadmap",
        title: "Roadmap",
        introduction: "This page separates completed repository work from work in progress, proposals still being evaluated, and directions intentionally kept out of scope. Roadmap Shipped can include work implemented in a source candidate; the install page and homepage separately identify the latest published package. Release records and GitHub issues refresh these categories when available.",
        sectionLabel: "Current status",
        sectionTitle: "Work grouped by status",
        browseIssues: "Browse open issues ↗",
        count: (value: number) => `${value} ${value === 1 ? "item" : "items"}`,
        contributeLabel: "Contribute",
        contributeTitle: "Keep roadmap decisions in the open.",
        contributeBody: "Use issues for bugs and well-scoped feature requests, Discussions for ideas that need shaping, and pull requests for concrete changes with tests or documentation. Verification across languages, platforms, and providers helps maintainers judge priority.",
        links: [
          { title: "Issues", detail: "Report a problem or propose scoped work.", href: "https://github.com/Hmbown/CodeWhale/issues" },
          { title: "Discussions", detail: "Explore an early idea before implementation.", href: "https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas" },
          { title: "Pull requests", detail: "Review existing work or send a focused change.", href: "https://github.com/Hmbown/CodeWhale/pulls" },
        ],
      };

  return (
    <div className="roadmap-page">
      <section className="hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container community-welcome-inner">
          <div className="eyebrow">{copy.eyebrow}</div>
          <h1>{copy.title}</h1>
          <p>{copy.introduction}</p>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container">
          <div className="portal-docs-heading">
            <div>
              <span>{copy.sectionLabel}</span>
              <h2>{copy.sectionTitle}</h2>
            </div>
            <Link href="https://github.com/Hmbown/CodeWhale/issues">{copy.browseIssues}</Link>
          </div>
          {tracks.map((track) => (
            <section key={track.title} className="portal-section-grid py-10 hairline-t">
              <div className="portal-section-copy">
                <span>{copy.count(track.items.length)}</span>
                <h2>{track.title}</h2>
              </div>
              <ul className="hairline-t">
                {track.items.map((item) => (
                  <li key={`${item.title}-${item.note}`} className="py-4 hairline-b">
                    <h3 className="font-display text-base">{roadmapText(item.title)}</h3>
                    <p className={`mt-1 text-sm text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                      {roadmapText(item.note)}
                    </p>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      </section>

      <section className="portal-section portal-section-muted">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>{copy.contributeLabel}</span>
            <h2>{copy.contributeTitle}</h2>
            <p>{copy.contributeBody}</p>
          </div>
          <div className="portal-topic-list">
            {copy.links.map((link) => (
              <Link key={link.title} href={link.href}>
                <strong>{link.title}</strong>
                <span>{link.detail}</span>
                <span aria-hidden="true">↗</span>
              </Link>
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}
