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
      ? "到 2026-07-14 为止的 CodeWhale 高节奏发布、桌面端和 Link/Tailscale 路线图。"
      : "CodeWhale's high-cadence release, desktop, and Link/Tailscale roadmap through July 14, 2026.",
  };
}

type TrackItem = Pick<RoadmapItem, "title" | "note" | "href">;
type RoadmapTrack = {
  title: string;
  cn: string;
  color: string;
  items: TrackItem[];
};

const tracksEn: RoadmapTrack[] = [
  {
    title: "Shipped",
    cn: "已完成",
    color: "jade",
    items: [
      { title: "v0.8.64 security and release hardening", note: "Latest shipped release, 2026-06-22: trust-boundary fixes, redaction, network safety, app-server cleanup, ACP history repair, and release CI repair.", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.64" },
      { title: "Sub-agent fanout safeguards", note: "v0.8.63 shipped bounded admission, per-worker token budgets, status/peek/cancel, provider-specific fanout config, and worktree isolation.", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.63" },
      { title: "Fleet real-run cutover", note: "v0.8.60 made `codewhale fleet run` launch durable profiled workers instead of staying as a planning shell.", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.60" },
      { title: "WhaleFlow foundations", note: "v0.8.60-v0.8.61 shipped declarative JS/TS authoring, runtime profiles, provider readiness, context-budget, adapter, registry, and telemetry foundations.", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.61" },
      { title: "Provider routing and fallback baseline", note: "v0.8.59-v0.8.62 shipped provider fallback state, centralized provider metadata, cross-provider model catalog hydration, GLM/StepFun/MiniMax/Hugging Face/DeepInfra/Kimi routes, and per-session provider/model isolation.", href: "https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md" },
      { title: "Durable goals, rewind, and local APIs", note: "Goal mode with verifier-as-judge, persistent thread-goal state, snapshot restore, and OpenAI-compatible app-server chat completions are already in the release train.", href: "https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md" },
    ],
  },
  {
    title: "Underway",
    cn: "进行中",
    color: "ochre",
    items: [
      { title: "Current shipped-milestone cleanup", note: "Burn down the still-open `v0.8.62`, `v0.8.63`, and `v0.8.64` issue leftovers by closing done work, moving real follow-ups, or cutting AM/PM patch releases.", href: "https://github.com/Hmbown/CodeWhale/issues?q=is%3Aissue+is%3Aopen" },
      { title: "v0.8.65 provider/model/offering refactor", note: "Split provider facts, model facts, offerings, and route resolution; route every provider/model switch through a resolved candidate; add provider-scoped pricing and usage.", href: "https://github.com/Hmbown/CodeWhale/milestone/50" },
      { title: "v0.8.66 token/cache discipline and Hotbar keys", note: "Token/cache/context regression fixtures, Hotbar MVP, default bindings, focus/modal key-dispatch coverage, approval semantics, terminal visual QA, and repo-context drift guards.", href: "https://github.com/Hmbown/CodeWhale/milestone/51" },
      { title: "v0.8.67 setup wizard and configuration hub", note: "Provider/model setup, trust and sandbox choices, tools/MCP/skills setup, remote/mobile/chat bridge setup, persistence, migration, and release QA.", href: "https://github.com/Hmbown/CodeWhale/milestone/52" },
      { title: "v0.8.68 TUI reliability and action-source follow-through", note: "Terminal rendering QA, Hotbar bindable sources, slash-command/workbench refactors, and user-reported output/input reliability issues.", href: "https://github.com/Hmbown/CodeWhale/milestone/53" },
    ],
  },
  {
    title: "Queued",
    cn: "排队中",
    color: "cobalt",
    items: [
      { title: "v0.8.69 website, docs, and distribution migration", note: "CodeWhale-native install/update names, website roadmap cleanup, website docs parity, community credit surfaces, and selected platform/search backlog cleanup.", href: "https://github.com/Hmbown/CodeWhale/milestone/54" },
      { title: "v0.8.70 display and reasoning-output reliability", note: "Stuck turns, truncated output, thinking-block rendering, terminal inspection affordances, and Windows/terminal backlog fixes.", href: "https://github.com/Hmbown/CodeWhale/milestone/55" },
      { title: "v0.8.71 legacy follow-up and dead-code inventory", note: "Delete, wire, or explicitly track migration, sandbox, release, i18n, profile-switching, connector, and stale compatibility issues.", href: "https://github.com/Hmbown/CodeWhale/milestone/56" },
      { title: "v0.8.72-v0.8.73 memory, fork UX, context, and keymap carryover", note: "Typed memory, fork UX, cache-maximal active-file behavior, configurable keymap, and Hotbar key follow-through.", href: "https://github.com/Hmbown/CodeWhale/milestone/57" },
      { title: "v0.9.0 multiplayer workrooms", note: "Chat-native CodeWhale rooms for threaded, shareable, multiplayer agent work: room state, presence, sessions, worker/fleet activity, workflow monitoring, replay, and stabilization.", href: "https://github.com/Hmbown/CodeWhale/issues/3209" },
    ],
  },
  {
    title: "Website + Launch",
    cn: "网站与发布",
    color: "cobalt",
    items: [
      { title: "Figma-inspired product surface", note: "Make codewhale.net feel like a mature product surface: crisp launch story, feature pages, product screenshots, changelog rhythm, docs CTAs, templates/examples, and community proof.", href: "https://github.com/Hmbown/CodeWhale/issues/3413" },
      { title: "Docs as managed product inventory", note: "Promote repo docs into website docs with parity checks, owner-friendly source of truth, drift checks, versioned launch notes, and no split-brain README/site claims.", href: "https://github.com/Hmbown/CodeWhale/issues/3417" },
      { title: "Roadmap and retired-plan hygiene", note: "Reconcile the public roadmap with retired web UI/share-link plans, multiplayer workrooms, desktop/link readiness, and the real AM/PM release train.", href: "https://github.com/Hmbown/CodeWhale/issues/3418" },
      { title: "Launch communication loop", note: "Publish community digest, multilingual contributor credit, install/update migration notes, release evidence, and launch-ready screenshots instead of one-off announcement churn.", href: "https://github.com/Hmbown/CodeWhale/issues/3420" },
      { title: "Website localization and support surface", note: "Keep website docs, README locales, troubleshooting, install paths, and support guidance in sync for the July 14 launch window.", href: "https://github.com/Hmbown/CodeWhale/issues/3090" },
    ],
  },
  {
    title: "Desktop + Link",
    cn: "桌面与连接",
    color: "indigo",
    items: [
      { title: "CodeWhale desktop app readiness", note: "Make the WeChat-like session OS the source of truth, wire local runtime controls, and gather desktop packaging evidence.", href: "https://github.com/Hmbown/codew/milestone/11" },
      { title: "Multiplayer room shell", note: "Use the desktop app as the first room surface: sessions/rooms, people/devices, agent activity, comments, handoff state, and honest degradation when runtime routes are missing.", href: "https://github.com/Hmbown/codew/issues/144" },
      { title: "Tailscale / trusted-LAN Link", note: "Desktop discovers the local runtime URL, guides Tailnet/LAN setup, exposes mobile link/QR, and records real-device LAN plus Tailnet smoke evidence.", href: "https://github.com/Hmbown/codew/issues/147" },
      { title: "Mobile control with safe approvals", note: "Health, sessions, transcript streaming, send/stop controls, notification poller, and read-only approvals until authenticated decision paths exist.", href: "https://github.com/Hmbown/codew/issues/150" },
      { title: "Runtime unlocks for devices and notifications", note: "Confirm or add the runtime routes for approvals, tokens, linked devices, and notifications that desktop/mobile need.", href: "https://github.com/Hmbown/codew/issues/146" },
      { title: "Voice layer plan", note: "On-device ASR/TTS by default, cloud providers capability-gated, and privacy boundaries explicit before mobile/desktop voice becomes a release promise.", href: "https://github.com/Hmbown/codew/issues/156" },
    ],
  },
  {
    title: "Ruled out",
    cn: "暂不考虑",
    color: "ink-mute",
    items: [
      { title: "Telemetry or phone-home by default", note: "The agent runs on your machine; release work should not make local runs report private workspace data." },
      { title: "Hosted SaaS dashboard as the default workflow", note: "The terminal remains the primary product surface; cloud or workroom paths must be explicit opt-in tracks." },
      { title: "Required login or account gate", note: "Bring your own provider key or local runtime. Core use should not require a CodeWhale account." },
      { title: "Sponsored model promotion", note: "Provider and model selection stays neutral; no paid placement in the picker or route resolver." },
    ],
  },
];

const tracksZh: RoadmapTrack[] = [
  {
    title: "已完成",
    cn: "Shipped",
    color: "jade",
    items: [
      { title: "v0.8.64 安全与发布加固", note: "最新已发布版本，2026-06-22：信任边界修复、日志脱敏、网络安全、app-server 清理、ACP 历史修复和发布 CI 修复。", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.64" },
      { title: "子 Agent 扇出保护", note: "v0.8.63 已发布有界接纳、单 worker token 预算、status/peek/cancel、按 provider 的扇出配置和 worktree 隔离。", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.63" },
      { title: "Fleet 真实运行切换", note: "v0.8.60 已让 `codewhale fleet run` 启动持久化、带 profile 的 worker，而不是只停留在规划壳层。", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.60" },
      { title: "WhaleFlow 基础", note: "v0.8.60-v0.8.61 已发布 JS/TS 声明式编排、运行时 profile、provider readiness、context budget、adapter、registry 和 telemetry 基础。", href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.61" },
      { title: "Provider 路由与 fallback 基线", note: "v0.8.59-v0.8.62 已发布 provider fallback、集中元数据、跨 provider 模型目录、GLM/StepFun/MiniMax/Hugging Face/DeepInfra/Kimi 路由，以及会话级 provider/model 隔离。", href: "https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md" },
      { title: "持久目标、回滚和本地 API", note: "带 verifier-as-judge 的 goal mode、持久 thread-goal 状态、快照恢复，以及 app-server 的 OpenAI 兼容 chat completions 已在发布列车中落地。", href: "https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md" },
    ],
  },
  {
    title: "进行中",
    cn: "Underway",
    color: "ochre",
    items: [
      { title: "当前已发布 milestone 的遗留清理", note: "清掉仍挂在 `v0.8.62`、`v0.8.63`、`v0.8.64` 上的开放 issue：已完成的关闭，真实后续移动，需要补丁的进入 AM/PM 小版本。", href: "https://github.com/Hmbown/CodeWhale/issues?q=is%3Aissue+is%3Aopen" },
      { title: "v0.8.65 provider/model/offering 重构", note: "拆分 provider facts、model facts、offerings 和 route resolution；所有 provider/model 切换先解析为候选路由；加入按 provider/offering 的 pricing 与 usage。", href: "https://github.com/Hmbown/CodeWhale/milestone/50" },
      { title: "v0.8.66 token/cache 纪律与 Hotbar keys", note: "Token/cache/context 回归夹具、Hotbar MVP、默认 bindings、focus/modal key-dispatch 覆盖、审批语义、终端视觉 QA 和 repo context drift guard。", href: "https://github.com/Hmbown/CodeWhale/milestone/51" },
      { title: "v0.8.67 setup wizard 与配置中心", note: "Provider/model 设置、信任与沙箱选择、tools/MCP/skills、remote/mobile/chat bridge 设置、持久化、迁移和发布 QA。", href: "https://github.com/Hmbown/CodeWhale/milestone/52" },
      { title: "v0.8.68 TUI 可靠性与 action-source 收尾", note: "终端渲染 QA、Hotbar 可绑定来源、slash-command/workbench 重构，以及用户报告的输出/输入可靠性问题。", href: "https://github.com/Hmbown/CodeWhale/milestone/53" },
    ],
  },
  {
    title: "排队中",
    cn: "Queued",
    color: "cobalt",
    items: [
      { title: "v0.8.69 网站、文档和分发命名迁移", note: "CodeWhale 原生命名的安装/更新路径、网站 roadmap 清理、网站文档 parity、社区 credit surface，以及部分平台/search backlog。", href: "https://github.com/Hmbown/CodeWhale/milestone/54" },
      { title: "v0.8.70 展示与 reasoning-output 可靠性", note: "卡住的 turn、截断输出、thinking-block 渲染、终端 inspection affordances，以及 Windows/终端 backlog。", href: "https://github.com/Hmbown/CodeWhale/milestone/55" },
      { title: "v0.8.71 legacy follow-up 与 dead-code inventory", note: "删除、接线或明确跟踪 migration、sandbox、release、i18n、profile-switching、connector 和 stale compatibility 问题。", href: "https://github.com/Hmbown/CodeWhale/milestone/56" },
      { title: "v0.8.72-v0.8.73 memory、fork UX、context 与 keymap 收尾", note: "Typed memory、fork UX、cache-maximal active-file 行为、configurable keymap 和 Hotbar key follow-through。", href: "https://github.com/Hmbown/CodeWhale/milestone/57" },
      { title: "v0.9.0 multiplayer workrooms", note: "面向多人协作的 chat-native CodeWhale rooms：room state、presence、sessions、worker/fleet activity、workflow monitoring、replay 和稳定化。", href: "https://github.com/Hmbown/CodeWhale/issues/3209" },
    ],
  },
  {
    title: "网站与发布",
    cn: "Website + Launch",
    color: "cobalt",
    items: [
      { title: "借鉴 Figma 的产品表面", note: "让 codewhale.net 像成熟产品：清晰 launch story、feature pages、产品截图、changelog 节奏、docs CTA、templates/examples 和社区证明。", href: "https://github.com/Hmbown/CodeWhale/issues/3413" },
      { title: "Docs 作为可管理的产品库存", note: "把 repo docs 推成网站 docs，带 parity checks、owner-friendly source of truth、drift checks、版本化 launch notes，并避免 README/site split-brain。", href: "https://github.com/Hmbown/CodeWhale/issues/3417" },
      { title: "Roadmap 与 retired-plan hygiene", note: "把 public roadmap 和已退休的 web UI/share-link 计划、multiplayer workrooms、desktop/link readiness、真实 AM/PM release train 对齐。", href: "https://github.com/Hmbown/CodeWhale/issues/3418" },
      { title: "Launch communication loop", note: "发布 community digest、多语言 contributor credit、install/update migration notes、release evidence 和 launch-ready screenshots，而不是一次性公告噪音。", href: "https://github.com/Hmbown/CodeWhale/issues/3420" },
      { title: "网站本地化与支持表面", note: "在 7 月 14 发布窗口内，让 website docs、README locales、troubleshooting、install paths 和 support guidance 保持同步。", href: "https://github.com/Hmbown/CodeWhale/issues/3090" },
    ],
  },
  {
    title: "桌面与连接",
    cn: "Desktop + Link",
    color: "indigo",
    items: [
      { title: "CodeWhale desktop app readiness", note: "把 WeChat-like session OS 作为产品 source of truth，接上本地 runtime controls，并补齐桌面端打包证据。", href: "https://github.com/Hmbown/codew/milestone/11" },
      { title: "Multiplayer room shell", note: "把桌面端作为第一个 room surface：sessions/rooms、people/devices、agent activity、comments、handoff state；runtime routes 缺失时诚实降级。", href: "https://github.com/Hmbown/codew/issues/144" },
      { title: "Tailscale / trusted-LAN Link", note: "桌面端发现本地 runtime URL，引导 Tailnet/LAN 设置，展示 mobile link/QR，并留下真机 LAN 与 Tailnet smoke evidence。", href: "https://github.com/Hmbown/codew/issues/147" },
      { title: "安全审批下的移动端控制", note: "Health、sessions、transcript streaming、send/stop controls、notification poller；在认证决策路径存在前，移动审批保持只读。", href: "https://github.com/Hmbown/codew/issues/150" },
      { title: "Devices 与 notifications 的 runtime unlock", note: "确认或新增桌面/移动端需要的 approvals、tokens、linked devices 和 notifications runtime routes。", href: "https://github.com/Hmbown/codew/issues/146" },
      { title: "Voice layer plan", note: "默认 on-device ASR/TTS，云 provider 走 capability gate；移动/桌面 voice 成为发布承诺前先明确隐私边界。", href: "https://github.com/Hmbown/codew/issues/156" },
    ],
  },
  {
    title: "暂不考虑",
    cn: "Ruled out",
    color: "ink-mute",
    items: [
      { title: "默认遥测或 phone-home", note: "Agent 在你的机器上运行；发布工作不应让本地运行上报私有 workspace 数据。" },
      { title: "默认托管 SaaS 面板", note: "终端仍是主产品面；cloud 或 workroom 路径必须是明确 opt-in 的轨道。" },
      { title: "强制登录或账号门槛", note: "自带 provider key 或本地 runtime 即可。核心使用不应需要 CodeWhale 账号。" },
      { title: "赞助商模型推广", note: "Provider 和 model 选择保持中立；picker 或 route resolver 不做付费推荐位。" },
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
        Queued: feed.considered,
        Considered: feed.considered,
        "Ruled out": feed.ruledOut,
        已完成: feed.shipped,
        进行中: feed.underway,
        排队中: feed.considered,
        考虑中: feed.considered,
        暂不考虑: feed.ruledOut,
      };
      tracks = baseTracks.map((t) => {
        const live = liveByCategory[t.title];
        if (live && live.length > 0) {
          return { ...t, items: live.map((it) => ({ title: it.title, note: it.note, href: it.href })) };
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
              到 2026-07-14 为止，这里按真实节奏规划：需要时上午一版、下午一版。
              目标是清掉当前 CodeWhale issue list，同时把桌面端和 Link/Tailscale 路径推进到 release readiness。
              未列在此页的内容均可在{" "}
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
                        <div className="font-display text-base">
                          {it.href ? (
                            <Link href={it.href} className="body-link">
                              {it.title}
                            </Link>
                          ) : (
                            it.title
                          )}
                        </div>
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
                  路线图反映的是维护者当前的发布计划——但 PR 和有理有据的讨论会不断调整优先级。
                  带一个可运行的原型来，"排队中"就能变成"进行中"。
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
              Through July 14, 2026, this roadmap assumes the real cadence: one AM release and one PM
              release when useful. The goal is to burn down the current CodeWhale issue list while moving
              the desktop app and Link/Tailscale path to release readiness. Anything not on this page is
              fair game for{" "}
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
                        <div className="font-display text-base">
                          {it.href ? (
                            <Link href={it.href} className="body-link">
                              {it.title}
                            </Link>
                          ) : (
                            it.title
                          )}
                        </div>
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
                  The roadmap reflects what the maintainer currently plans to ship — but PRs and well-argued
                  discussions reorder it constantly. Show up with a working prototype and watch
                  "Queued" become "Underway".
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
