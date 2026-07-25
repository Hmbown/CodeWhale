import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/work",
    locale,
    title: isZh ? "工作面板 · Codewhale 文档" : "Work Surface · Codewhale Docs",
    description: isZh
      ? "带计数的 To-do 执行台账、update_plan 策略上下文，以及同一份工作状态的延续路径。"
      : "The counted To-do execution ledger, update_plan strategy context, and how one work state stays continuous.",
  });
}

export default async function WorkSurfacePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "工作面板" : "The Work surface"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 的 TUI 侧栏有一块 Work 区域，显示当前工作的实时状态。它不只是视觉上的待办清单：同一份工作状态同时由模型可见的工具、会话接力（relay）和子 Agent 交接共同维护。它由两层构成——一个带计数的执行台账（To-do），和一层可选的策略上下文（update_plan 元数据）。"
            : "The TUI sidebar has a Work area that shows live state for the current job. It is more than a visual to-do list: the same work state is maintained by model-visible tools, session relay, and sub-agent handoff. It has two layers — a counted execution ledger (the To-do) and an optional strategy context (update_plan metadata)."}
        </p>
      </section>

      <section id="checklist" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "To-do：带计数的执行台账" : "Checklist: the counted execution ledger"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              To-do 是具体工作的进度台账：一组带状态的条目（pending / in_progress / completed /
              cancelled），外加完成百分比和当前进行中的条目。模型通过 canonical 的{" "}
              <code className="inline">work_update</code> 工具替换活动线程或持久任务的
              To-do 投影——这是模型可见的进度表面。旧的{" "}
              <code className="inline">checklist_*</code> 和 <code className="inline">todo_*</code>{" "}
              名字仍是隐藏的兼容别名：它们对同一份 To-do 状态保持可派发，以便旧 transcript
              回放，但不会出现在模型目录里。
            </>
          ) : (
            <>
              The To-do is the progress ledger for concrete work: a list of items with status
              (pending / in_progress / completed / cancelled), a completion percentage, and the item
              currently in progress. The model replaces this projection for the active thread or
              durable task through the canonical <code className="inline">work_update</code> tool —
              the model-visible progress surface. The legacy{" "}
              <code className="inline">checklist_*</code> and <code className="inline">todo_*</code>{" "}
              names remain hidden compatibility aliases: they stay dispatchable against the same To-do
              state so old transcripts replay, but they are not advertised to the model catalog.
            </>
          )}
        </p>
      </section>

      <section id="strategy" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "策略上下文：update_plan 元数据" : "Strategy context: update_plan metadata"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "update_plan 承载的是可选的高层策略，不是第二个清单。它的字段面向阶段级理解：标题、目标、上下文摘要、说明、来源、关键文件、约束、推荐方案、验证计划、风险与未知、交接包，以及一组步骤。它帮助父会话或后续 worker 理解“为什么这么做”；具体执行进度始终属于 To-do 台账。侧栏有意不把策略状态渲染成第二条进度列表——两份进度并存只会制造歧义。"
            : "update_plan carries optional high-level strategy — it is not a second checklist. Its fields serve phase-level understanding: title, objective, context summary, explanation, sources, critical files, constraints, recommended approach, verification plan, risks and unknowns, a handoff packet, and a list of steps. It helps a parent session or a later worker understand the approach; concrete execution progress always belongs to the To-do ledger. The sidebar deliberately does not render strategy state as a second progress list — two competing progress surfaces would only create ambiguity."}
        </p>
      </section>

      <section id="continuity" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "延续性：同一份状态流向各处" : "Continuity: one state, many surfaces"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "同一份工作状态喂给多个出口：侧栏的 To-do 区域实时渲染它；/relay 让模型把 To-do 快照和 update_plan 策略元数据写进交接文件，供下一个线程接续；分叉（fork_context）的子 Agent 会在其前缀里收到一份结构化状态块，其中的 Work 小节就是这份 To-do 快照——子 Agent 因此从父级真实的进度位置继续，而不是从转述的摘要开始。"
            : "The same work state feeds several surfaces: the sidebar's To-do area renders it live; /relay has the model write the To-do snapshot and the update_plan strategy metadata into a handoff artifact for the next thread; and a forked (fork_context) sub-agent receives a structured state block in its prefix whose Work section is this To-do snapshot — the child continues from the parent's real progress position instead of a paraphrased summary."}
        </p>
      </section>

      <section id="capture" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "终端实拍（文本复原）" : "Terminal capture (faithful text)"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "下面的文本块按 crates/tui/src/tui/sidebar.rs 的渲染逻辑逐行复原侧栏 Work 区域：目标是带 ◆ 图标的 Goal 行、耗时、token 预算条；然后是完成度计数和带编号的状态条目。"
            : "This text block reproduces the sidebar Work area line-for-line from the rendering logic in crates/tui/src/tui/sidebar.rs: the goal row with its ◆ icon, elapsed time, and token budget bar, then the settled counter and the numbered status items."}
        </p>
        <pre className="code-block mt-4">{`To-do
◆ Goal: Land the v0.9.2 website docs cluster
elapsed: 18m
[█████████░░░░░░░░░░░] 45%
50% settled (2/4)
[✓] #1 Read docs-map.ts and the Modes page pattern
[✓] #2 Draft the Fleet and Sandbox pages
[~] #3 Write the Work surface page
[ ] #4 Run check:docs, tests, and the build`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              条目前缀对应四种状态：<code className="inline">[ ]</code> 待办、
              <code className="inline">[~]</code> 进行中、<code className="inline">[✓]</code> 完成、
              <code className="inline">[-]</code> 取消。空间不够时侧栏窗口化到进行中条目附近，并用
              “+N more To-do items” 标注被省略的条目。
            </>
          ) : (
            <>
              The item prefixes map to the four statuses: <code className="inline">[ ]</code> pending,{" "}
              <code className="inline">[~]</code> in progress, <code className="inline">[✓]</code>{" "}
              completed, <code className="inline">[-]</code> cancelled. When space runs out, the sidebar
              windows around the in-progress item and marks the omission with “+N more To-do items”.
            </>
          )}
        </p>
      </section>

      <section id="model-facing" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "哪些是模型可见的，哪些只是界面" : "What is model-facing vs. visual-only"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "已被实现和测试证实的模型可见路径有三条：work_update 工具本身是模型目录里的活跃工具；分叉子 Agent 的结构化状态块（<codewhale:fork_state> 中的 Work 小节，有针对它的引擎测试）；以及 /relay 输出，它把同一份 To-do 快照和策略元数据注入交接指令。侧栏渲染是视觉呈现——它给人看，不注入模型上下文。"
            : "Three model-facing paths are implemented and covered by tests: the work_update tool itself, which is active in the model catalog; the forked sub-agent's structured state block (the Work section inside <codewhale:fork_state>, pinned by an engine test); and /relay output, which injects the same To-do snapshot and strategy metadata into the handoff instruction. The sidebar rendering is a visual presentation — it informs the operator and is not injected into model context."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              有一项能力我们刻意不宣称：把当前 Work 状态注入普通父回合的模型上下文（父回合级
              grounding）。它取决于{" "}
              <a
                href="https://github.com/Hmbown/CodeWhale/issues/3983"
                target="_blank"
                rel="noreferrer"
                className="body-link"
              >
                issue #3983
              </a>{" "}
              的运行时测试落地；在那之前，本页只描述已证实的行为。
            </>
          ) : (
            <>
              One capability is deliberately not claimed: injecting the current Work state into a
              normal parent turn's model context (parent-turn grounding). That depends on the runtime
              test tracked in{" "}
              <a
                href="https://github.com/Hmbown/CodeWhale/issues/3983"
                target="_blank"
                rel="noreferrer"
                className="body-link"
              >
                issue #3983
              </a>
              ; until it lands, this page describes only proven behavior.
            </>
          )}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/TOOL_SURFACE.md, docs/TOOL_LIFECYCLE.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/TOOL_SURFACE.md, docs/TOOL_LIFECYCLE.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
