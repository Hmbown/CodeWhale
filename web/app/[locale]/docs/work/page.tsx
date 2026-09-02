import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/work",
    locale,
    title: isZh ? "工作面板 · Codewhale 文档" : "Work Surface · Codewhale Docs",
    description: isZh
      ? "唯一的 To-do 列表、模型如何看到它，以及同一份工作状态的延续路径。"
      : "The single To-do list, how the model sees it, and how one work state stays continuous.",
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
        <h1 className="font-display text-3xl mb-1">{isZh ? "工作面板" : "The Work surface"}</h1>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 的 TUI 侧栏有一块 Work 区域，显示当前工作的实时状态。它不只是视觉上的待办清单：同一份工作状态同时由模型可见的工具、会话接力（relay）和子 Agent 交接共同维护。Codewhale 只有一个 Work 面板——带计数的 To-do 执行台账。update_plan 是对话式的推理笔记，不是第二个进度面板。"
            : "The TUI sidebar has a Work area that shows live state for the current job. It is more than a visual to-do list: the same work state is maintained by model-visible tools, session relay, and sub-agent handoff. Codewhale has exactly one Work surface — the counted To-do execution ledger. update_plan is conversational reasoning, not a second progress surface."}
        </p>
      </section>

      <section id="checklist" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "To-do：唯一的执行台账" : "To-do: the sole canonical ledger"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              To-do 是具体工作的进度台账：一组带状态的条目（pending / in_progress / completed /
              cancelled），外加完成百分比和当前进行中的条目。模型通过 canonical 的{" "}
              <code className="inline">todo_write</code> 工具替换活动线程或持久任务的
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
              durable task through the canonical <code className="inline">todo_write</code> tool —
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
          {isZh ? "策略是对话式推理：update_plan" : "Strategy is conversational reasoning: update_plan"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "update_plan 承载的是可选的高层策略，不是第二份清单。它的字段面向阶段级理解：标题、目标、上下文摘要、说明、来源、关键文件、约束、推荐方案、验证计划、风险与未知、交接包，以及一组步骤。它帮助父会话或后续 worker 理解“为什么这么做”；具体执行进度始终属于 To-do 列表。侧栏有意不把策略状态渲染成第二条进度列表，各个 To-do 快照出口也不会包含它——只有 update_plan 而 To-do 为空时，不会产生任何 To-do 快照。"
            : "update_plan carries optional high-level strategy — it is not a second list. Its fields serve phase-level understanding: title, objective, context summary, explanation, sources, critical files, constraints, recommended approach, verification plan, risks and unknowns, a handoff packet, and a list of steps. It helps a parent session or a later worker understand the approach; concrete execution progress always belongs to the To-do list. The sidebar deliberately does not render strategy state as a second progress list, and neither does any To-do snapshot surface — plan state with an empty To-do produces no snapshot at all."}
        </p>
      </section>

      <section id="continuity" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "延续性：同一份状态流向各处" : "Continuity: one state, many surfaces"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "模型通过自己的工具结果了解 To-do：todo_write 返回的结果就是普通的会话历史，因此无需在每一步重复注入清单。只有在有人明确要求的节点，才会用同一个渲染器展示一次当前 To-do：分叉（fork_context）的子 Agent 在其结构化状态块里收到该正文；/relay 把同样的正文写进交接指令。两处的 To-do 正文逐字节一致——子 Agent 与下一个线程因此从父级真实的进度位置继续，而不是从转述的摘要开始。侧栏的 To-do 区域则完整实时渲染同一份状态。"
            : "The model learns the To-do from its own tool results: what todo_write returned is ordinary conversation history, so nothing re-states the list on each step. Only at a seam a person asked for does one renderer show the current To-do once: a forked (fork_context) sub-agent receives that body inside its structured state block, and /relay writes the same body into the handoff instruction. The To-do body is byte-identical in both, so a child agent and the next thread continue from the parent's real progress position instead of a paraphrased summary. The sidebar renders that same state live, in full."}
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
            ? "已被实现和测试证实的模型可见路径有三条：todo_write 工具本身是模型目录里的活跃工具，它返回的工具结果就是模型看到清单的方式；分叉子 Agent 的结构化状态块（<codewhale:fork_state> 中的 To-do 小节，在真正 fork 的那一刻解析）；以及 /relay 输出。没有任何一步请求会重复注入 To-do——一条结构化测试直接断言真实出站请求体里不含该清单。侧栏渲染是视觉呈现——它给人看，不注入模型上下文。"
            : "Three model-facing paths are implemented and covered by tests: the todo_write tool itself, which is active in the model catalog and whose tool result is how the model sees the list; the forked sub-agent's structured state block (the To-do section inside <codewhale:fork_state>, resolved at the moment of the fork); and /relay output. No request re-states the To-do on any step — a structural test asserts the real outbound request body does not contain the list. The sidebar rendering is a visual presentation — it informs the operator and is not injected into model context."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "边界值得说清楚：因为没有逐步注入，稳定的系统与工具前缀完全不受 To-do 变化影响，前缀缓存也不会因此失效。fork 那一次的快照读取的是权威状态（有 work graph 时读它暂存的投影，而不是尚未发布的旧视图），所以同一回合里较早的一次 todo_write 也会被带上。条目数与字符数都有硬上限，进行中的条目优先保留，被省略的部分带省略标记。To-do 为空时不输出任何内容。渲染器只保证包裹结构、控制字符与上限这三件事——它不会审查条目文本的含义，任意 To-do 内容不因此变成可信指令。"
            : "The boundaries are worth stating: because nothing is injected per step, the stable system-and-tool prefix is untouched by To-do changes and prefix caching is never invalidated by them. The one snapshot taken at a fork reads the authoritative state (the work graph's staged projection where one exists, not the not-yet-published legacy view), so a todo_write made earlier in the same turn is included. Item count and character count are both hard-bounded, the in-progress item is preserved preferentially, and elided content is marked. An empty To-do emits nothing at all. The renderer guarantees exactly three things — wrapper framing cannot be closed early, control characters cannot forge the line format, and the bounds hold. It does not vet what item text says, so arbitrary To-do content is not thereby made safe to follow as instructions."}
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
