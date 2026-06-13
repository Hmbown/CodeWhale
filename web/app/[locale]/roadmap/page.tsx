import Link from "next/link";
import { Seal } from "@/components/seal";
import { getCachedRoadmap, type RoadmapItem } from "@/lib/roadmap-feed";
import { getEnv } from "@/lib/kv";

export const revalidate = 1800;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "路线图 · CodeWhale" : "Roadmap · CodeWhale",
    description: isZh
      ? "已确认、正在评估和已排除的功能规划。"
      : "What's confirmed, what's being weighed, what's been ruled out for CodeWhale.",
  };
}

const tracksEn = [
  {
    title: "Shipped",
    cn: "已完成",
    color: "jade",
    items: [
      { title: "Typed tool surface", note: "read, write, edit, patch, grep, shell, git, web search — plus sub-agents, RLM, and MCP" },
      { title: "Sub-agent parallel execution", note: "agent_open / agent_eval / agent_close; up to 10 concurrent sessions with bounded result handles" },
      { title: "RLM batched processing", note: "Persistent sandboxed Python REPL with 1–16 cheap parallel children for long-input analysis" },
      { title: "Three operating modes", note: "Plan (read-only), Agent (default), YOLO (auto-approved); orthogonal suggest / auto / never approval" },
      { title: "Per-platform sandbox", note: "seatbelt (macOS), landlock (Linux); Windows containment via restricted tokens (limited)" },
      { title: "Durable sessions + tasks", note: "Save, resume, rollback; background task queue with replayable timelines under ~/.codewhale/tasks/" },
      { title: "Bidirectional MCP", note: "Consume tools from external servers; expose as server via `codewhale mcp`; ~/.codewhale/mcp.json" },
      { title: "Skills + unified slash palette", note: "~/.codewhale/skills/ auto-loading; /help, /mode, /status, /config, /trust, /feedback" },
      { title: "OpenRouter provider", note: "First-class OpenRouter integration with 300+ models across dozens of providers" },
      { title: "Multi-provider support", note: "Hot-swap between providers (DeepSeek, OpenAI, Anthropic, OpenRouter) per session" },
    ],
  },
  {
    title: "Underway",
    cn: "进行中",
    color: "ochre",
    items: [
      { title: "VS Code extension", note: "Scaffold, local runtime detection, chat webview — first external editor surface (#461–#463)" },
      { title: "Memory typed store", note: "SQLite + FTS5 backend with graph-structured agent memory and multi-signal recall (#534–#536)" },
      { title: "Feishu / Lark bot", note: "Chat-platform frontend over the existing runtime API (#757)" },
      { title: "Chinese-market & i18n", note: "Locale-aware UI, platform refinements, region-specific search backends (#755)" },
      { title: "Hugging Face model discovery + Model Lab", note: "Browse, download, and manage models from Hugging Face Hub directly in the TUI" },
      { title: "ZenMux / OpenAI-compatible providers", note: "Bring any OpenAI-compatible endpoint (vLLM, LiteLLM, Ollama, local) as a first-class provider" },
    ],
  },
  {
    title: "Considered",
    cn: "考虑中",
    color: "cobalt",
    items: [
      { title: "Web UI / share-link mode", note: "Local web interface over serve --http; curated, generated static share links (#471, #481)" },
      { title: "Exa web-search backend", note: "Bundled alternative to the existing DDG + Bing path (#431)" },
      { title: "Homebrew core formula", note: "Tap exists; pursuing homebrew-core inclusion" },
      { title: "Native Windows installer", note: "MSI / WinGet; Scoop manifest already ships" },
      { title: "Unsloth / NeMo / Arcee fine-tune integration", note: "One-click fine-tuning workflows backed by Unsloth, NVIDIA NeMo, and Arcee toolkits" },
    ],
  },
  {
    title: "Ruled out",
    cn: "暂不考虑",
    color: "ink-mute",
    items: [
      { title: "Telemetry / phone-home", note: "The agent runs on your machine — what happens there stays there" },
      { title: "Hosted SaaS dashboard", note: "The terminal IS the dashboard; the website is informational only" },
      { title: "Required login / accounts", note: "Bring your own API key, that's it" },
      { title: "Sponsored model promotion", note: "Model picker stays neutral — no paid placement" },
    ],
  },
  {
    title: "Open model platform",
    cn: "开放模型平台",
    color: "indigo",
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
    cn: "Shipped",
    color: "jade",
    items: [
      { title: "类型化工具集", note: "文件读写、编辑、补丁、搜索、Shell、Git、子 Agent、RLM、MCP——覆盖日常开发全流程" },
      { title: "子 Agent 并行执行", note: "agent_open / agent_eval / agent_close；最多 10 个并发会话，通过 var_handle 有界读取结果" },
      { title: "RLM 批量处理", note: "持久沙箱 Python REPL，支持 1–16 路廉价并行子调用，处理长文本分析" },
      { title: "三种运行模式", note: "Plan（只读）、Agent（默认）、YOLO（自动批准）；审批模式正交（建议/自动/拒绝）" },
      { title: "跨平台沙箱", note: "seatbelt（macOS）、landlock（Linux）；Windows 通过受限令牌实现基础隔离（功能有限）" },
      { title: "持久化会话 + 后台任务", note: "保存、恢复、回滚；后台任务队列，可回放时间线，位于 ~/.codewhale/tasks/" },
      { title: "双向 MCP 协议", note: "消费外部服务器工具；通过 `codewhale mcp` 暴露为服务器；~/.codewhale/mcp.json" },
      { title: "技能 + 统一命令面板", note: "~/.codewhale/skills/ 自动加载；/help、/mode、/status、/config、/trust、/feedback" },
      { title: "OpenRouter 提供商", note: "原生集成 OpenRouter，支持 300+ 模型，覆盖数十个提供商" },
      { title: "多提供商支持", note: "按会话动态切换提供商（DeepSeek、OpenAI、Anthropic、OpenRouter）" },
    ],
  },
  {
    title: "进行中",
    cn: "Underway",
    color: "ochre",
    items: [
      { title: "VS Code 扩展", note: "脚手架、本地运行时检测、聊天 Webview——首个外部编辑器集成（#461–#463）" },
      { title: "记忆类型化存储", note: "SQLite + FTS5 后端，图结构 Agent 记忆，多信号召回（#534–#536）" },
      { title: "飞书 / Lark 机器人", note: "基于现有 runtime API 的聊天平台前端（#757）" },
      { title: "中国市场与国际化改进", note: "本地化 UI、平台优化、区域搜索引擎（#755）" },
      { title: "Hugging Face 模型发现 + 模型实验室", note: "在 TUI 中直接浏览、下载和管理 Hugging Face Hub 上的模型" },
      { title: "ZenMux / OpenAI 兼容提供商", note: "将任意 OpenAI 兼容端点（vLLM、LiteLLM、Ollama、本地模型）作为一级提供商接入" },
    ],
  },
  {
    title: "考虑中",
    cn: "Considered",
    color: "cobalt",
    items: [
      { title: "Web 界面 / 分享链接模式", note: "通过 serve --http 提供本地 Web 界面；精选静态分享链接（#471、#481）" },
      { title: "Exa 网页搜索后端", note: "内建替代 DDG + Bing 的搜索路由（#431）" },
      { title: "Homebrew 核心仓库", note: "Tap 已有；正在争取进入 homebrew-core" },
      { title: "Windows 原生安装器", note: "MSI / WinGet；Scoop 清单已发布" },
      { title: "Unsloth / NeMo / Arcee 微调集成", note: "一键微调工作流，由 Unsloth、NVIDIA NeMo 和 Arcee 工具链驱动" },
    ],
  },
  {
    title: "暂不考虑",
    cn: "Ruled out",
    color: "ink-mute",
    items: [
      { title: "遥测 / 回传数据", note: "Agent 在你的机器上运行——你的数据不会离开" },
      { title: "托管 SaaS 面板", note: "终端即面板；网站仅供信息展示" },
      { title: "强制登录 / 注册", note: "自带 API 密钥即可" },
      { title: "赞助商模型推广", note: "模型选择器保持中立——无付费推荐位" },
    ],
  },
  {
    title: "开放模型平台",
    cn: "Open model platform",
    color: "indigo",
    items: [
      { title: "社区模型注册中心", note: "发现、分享和评价社区微调模型，附带可复现的配方" },
      { title: "一键部署", note: "一条命令将任意模型部署到 RunPod、Replicate 或自有基础设施" },
      { title: "模型评测面板", note: "跨提供商、量化级别和硬件的透明、可复现对比" },
    ],
  },
];

const colorFor = (c: string) =>
  c === "jade" ? "border-jade text-jade" :
  c === "ochre" ? "border-ochre text-ochre" :
  c === "cobalt" ? "border-cobalt text-cobalt" :
  c === "indigo" ? "border-indigo text-indigo" :
  "border-ink-mute text-ink-mute";

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

  return (
    <>
      {isZh ? (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="路" />
              <div className="eyebrow">Section 04 · 路线</div>
            </div>
            <h1 className="font-display tracking-crisp">
              路线图 <span className="font-cjk text-indigo text-5xl ml-2">Roadmap</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
              已确认的功能、正在权衡的方案、以及已被排除的方向。未列在此页的内容均可在{" "}
              <Link href="https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas" className="body-link">
                Discussions
              </Link>{" "}
              中讨论。
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-20 grid lg:grid-cols-2 gap-px bg-paper-line">
            {tracks.map((t) => (
              <div key={t.title} className="bg-paper p-7">
                <div className={`hairline-b pb-3 mb-5 flex items-baseline justify-between border-b-2 ${colorFor(t.color)}`}>
                  <div>
                    <h2 className="font-display text-3xl">
                      {t.title} <span className="font-cjk text-2xl ml-2 text-ink-mute">{t.cn}</span>
                    </h2>
                  </div>
                  <div className="font-mono text-xs uppercase tracking-widest tabular text-ink-mute">{t.items.length} 项</div>
                </div>
                <ul className="space-y-4">
                  {t.items.map((it, i) => (
                    <li key={i} className="flex gap-4">
                      <span className={`font-display text-xl tabular shrink-0 w-8 ${colorFor(t.color)}`}>{String(i + 1).padStart(2, "0")}</span>
                      <div>
                        <div className="font-display text-base">{it.title}</div>
                        <div className="text-sm text-ink-soft mt-0.5 leading-[1.9] tracking-wide">{it.note}</div>
                      </div>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </section>

          <section className="bg-ink text-paper">
            <div className="mx-auto max-w-[1400px] px-6 py-12 grid lg:grid-cols-12 gap-6 items-center">
              <div className="lg:col-span-8">
                <div className="font-cjk text-indigo text-lg mb-2">参与塑造</div>
                <h2 className="font-display text-paper text-3xl">想影响这份清单？</h2>
                <p className="mt-3 text-paper-deep/80 leading-[1.9] tracking-wide max-w-2xl">
                  路线图反映的是维护者的计划——但 PR 和有理有据的讨论会不断调整优先级。
                  带一个可运行的原型来，"考虑中"就能变成"进行中"。
                </p>
              </div>
              <div className="lg:col-span-4 flex flex-col gap-3">
                <Link
                  href="https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas"
                  className="px-5 py-3 bg-indigo text-paper font-mono text-sm uppercase tracking-wider text-center hover:bg-indigo-deep transition-colors"
                >
                  提交想法 →
                </Link>
                <Link
                  href="https://github.com/Hmbown/CodeWhale/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22"
                  className="px-5 py-3 hairline-t hairline-b hairline-l hairline-r border-paper-deep/30 font-mono text-sm uppercase tracking-wider text-center hover:bg-paper hover:text-ink transition-colors"
                >
                  Good first issues →
                </Link>
              </div>
            </div>
          </section>
        </>
      ) : (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="路" />
              <div className="eyebrow">Section 04 · 路线</div>
            </div>
            <h1 className="font-display tracking-crisp">
              Roadmap <span className="font-cjk text-indigo text-5xl ml-2">路线图</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-relaxed">
              What's confirmed, what's being weighed, what's been ruled out. Anything not on this page
              is fair game for{" "}
              <Link href="https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas" className="body-link">
                discussion
              </Link>.
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-20 grid lg:grid-cols-2 gap-px bg-paper-line">
            {tracks.map((t) => (
              <div key={t.title} className="bg-paper p-7">
                <div className={`hairline-b pb-3 mb-5 flex items-baseline justify-between border-b-2 ${colorFor(t.color)}`}>
                  <div>
                    <h2 className="font-display text-3xl">
                      {t.title} <span className="font-cjk text-2xl ml-2 text-ink-mute">{t.cn}</span>
                    </h2>
                  </div>
                  <div className="font-mono text-xs uppercase tracking-widest tabular text-ink-mute">{t.items.length} items</div>
                </div>
                <ul className="space-y-4">
                  {t.items.map((it, i) => (
                    <li key={i} className="flex gap-4">
                      <span className={`font-display text-xl tabular shrink-0 w-8 ${colorFor(t.color)}`}>{String(i + 1).padStart(2, "0")}</span>
                      <div>
                        <div className="font-display text-base">{it.title}</div>
                        <div className="text-sm text-ink-soft mt-0.5 leading-relaxed">{it.note}</div>
                      </div>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </section>

          <section className="bg-ink text-paper">
            <div className="mx-auto max-w-[1400px] px-6 py-12 grid lg:grid-cols-12 gap-6 items-center">
              <div className="lg:col-span-8">
                <div className="font-cjk text-indigo text-lg mb-2">参与塑造</div>
                <h2 className="font-display text-paper text-3xl">Want to shape this list?</h2>
                <p className="mt-3 text-paper-deep/80 leading-relaxed max-w-2xl">
                  The roadmap reflects what the maintainer plans to do — but PRs and well-argued
                  discussions reorder it constantly. Show up with a working prototype and watch
                  "Considered" become "Underway".
                </p>
              </div>
              <div className="lg:col-span-4 flex flex-col gap-3">
                <Link
                  href="https://github.com/Hmbown/CodeWhale/discussions/new?category=ideas"
                  className="px-5 py-3 bg-indigo text-paper font-mono text-sm uppercase tracking-wider text-center hover:bg-indigo-deep transition-colors"
                >
                  Propose an idea →
                </Link>
                <Link
                  href="https://github.com/Hmbown/CodeWhale/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22"
                  className="px-5 py-3 hairline-t hairline-b hairline-l hairline-r border-paper-deep/30 font-mono text-sm uppercase tracking-wider text-center hover:bg-paper hover:text-ink transition-colors"
                >
                  Good first issues →
                </Link>
              </div>
            </div>
          </section>
        </>
      )}
    </>
  );
}
