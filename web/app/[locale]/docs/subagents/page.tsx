import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/subagents",
    locale,
    title: isZh ? "子 Agent · Codewhale 文档" : "Sub-Agents · Codewhale Docs",
    description: isZh
      ? "agent 工具、Fleet 角色、上下文分叉、worktree 隔离和并发上限。"
      : "The agent tool, Fleet roles, context forking, worktree isolation, and concurrency caps.",
  });
}

export default async function SubagentsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const roles = isZh
    ? [
        { name: "worker", detail: "灵活执行父级交代的多步任务；可写、可用 shell。默认角色。" },
        { name: "scout", detail: "只读，快速摸清相关代码——例如“找出 Foo 的所有调用点”。" },
        { name: "planner", detail: "分析并产出策略，不执行——“设计迁移方案，不要动手”。" },
        { name: "reviewer", detail: "只读审查并按严重度打分——“审一遍这个 PR 的 bug”。" },
        { name: "builder", detail: "以最小改动落地一个明确的变更；可写、可用 shell。" },
        { name: "verifier", detail: "运行测试和校验并汇报结果，不写代码。" },
        { name: "custom", detail: "手工指定狭窄的工具白名单，用于锁定的派发。" },
      ]
    : [
        { name: "worker", detail: "Flexible multi-step execution of the parent's brief; writes and shell allowed. The default role." },
        { name: "scout", detail: "Read-only, maps the relevant code fast — “find every call site of Foo.”" },
        { name: "planner", detail: "Analyse and produce a strategy without executing — “design the migration; don't run it.”" },
        { name: "reviewer", detail: "Read-and-grade with severity scores — “audit this PR for bugs.”" },
        { name: "builder", detail: "Land a specific change with minimal edits; writes and shell allowed." },
        { name: "verifier", detail: "Run tests and validation gates and report the outcome; no code edits." },
        { name: "custom", detail: "An explicit narrow tool allowlist for locked-down dispatch." },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "子 Agent" : "Sub-Agents"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "父会话通过 agent 工具启动一个有明确职责的子 Agent，并立即拿回 agent_id、compact 收据和 transcript 句柄；子 Agent 在后台运行。子 Agent 默认继承父级的工具注册表，但它们是叶子 worker：不会再拿到 agent 或嵌套生命周期工具。agent 启动的是分离的后台工作——取消父回合会停止父级的等待路径，但不会杀死已经启动的子运行。"
            : "A parent session launches one focused sub-agent through the agent tool and immediately gets back an agent_id, a compact receipt, and a transcript handle while the worker runs in the background. Sub-agents inherit the parent's tool registry by default, but they are leaf workers: they do not receive agent or nested lifecycle tools. agent launches detached background work — cancelling the parent turn stops the parent's wait path, but it does not kill already-opened child runs."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "对于必须跨进程重启、睡眠或远程执行存活的工作，优先选择 Fleet 或 Workflow 支撑的 Fleet 运行，而不是会话内的短寿命 agent 调用。"
            : "For work that must survive process restarts, sleep, or remote execution, prefer Fleet or a Workflow-backed fleet run over a short in-session agent call."}
        </p>
        <div className="hairline-t mt-6">
          {roles.map((row) => (
            <section key={row.name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{row.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="fork" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "上下文分叉" : "Context forking"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              <code className="inline">agent</code> 默认开启全新会话：子 Agent 只拿到角色提示词和你给的任务。当任务依赖父
              transcript 里已有的决定、文件、待办或计划状态时，用{" "}
              <code className="inline">fork_context: true</code>
              ——运行时在可用时保持父级前缀逐字节一致（保留前缀缓存复用），追加一份结构化状态快照，再把子
              Agent 的角色说明和任务放在末尾。独立探索用新会话，延续、审查、总结或压缩类工作用分叉会话。
            </>
          ) : (
            <>
              <code className="inline">agent</code> starts fresh by default: the child gets its role
              prompt plus the task you pass. When the task depends on decisions, files, todos, or plan
              state already in the parent transcript, use{" "}
              <code className="inline">fork_context: true</code> — the runtime keeps the parent's
              request prefix byte-identical where available (preserving prefix-cache reuse), appends a
              structured state snapshot, then adds the sub-agent role instructions and task at the tail.
              Use fresh sessions for independent exploration and forked sessions for continuation,
              review, summarization, or compaction work.
            </>
          )}
        </p>
      </section>

      <section id="worktree" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "Worktree 隔离" : "Worktree isolation"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              并行编辑通道用 <code className="inline">worktree: true</code> 启动：Codewhale
              为子 Agent 创建新的 git worktree 和分支（默认{" "}
              <code className="inline">codex/agent-&lt;name&gt;-&lt;id&gt;</code>，检出在父仓库旁的{" "}
              <code className="inline">.codewhale-worktrees/</code> 下），父检出保持干净。隔离不等于写权限：只带
              prompt 的 worker 从只读开始；要写代码的子 Agent 还需声明{" "}
              <code className="inline">write_authority</code> 和至少一个规范化的{" "}
              <code className="inline">write_roots</code>、<code className="inline">exact_files</code>{" "}
              或 <code className="inline">coordination_contracts</code>
              值；重叠的共享写声明会在任何改动之前失败。
            </>
          ) : (
            <>
              Launch parallel edit lanes with <code className="inline">worktree: true</code>: Codewhale
              creates a fresh git worktree and branch for the child (default{" "}
              <code className="inline">codex/agent-&lt;name&gt;-&lt;id&gt;</code>, checked out beside the
              parent repo under <code className="inline">.codewhale-worktrees/</code>) so the parent
              checkout stays clean. Isolation is not write authority: a prompt-only worker starts
              read-only, and a writer also declares{" "}
              <code className="inline">write_authority</code> plus at least one normalized{" "}
              <code className="inline">write_roots</code>, <code className="inline">exact_files</code>,
              or <code className="inline">coordination_contracts</code> value. Overlapping shared write
              claims fail before any mutation.
            </>
          )}
        </p>
      </section>

      <section id="capacity" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "并发上限" : "Concurrency caps"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "子 Agent 容量的权威来源是 crates/tui/src/config/subagent_limits.rs：默认配置并发 64，最大配置并发 128，运行加排队的最大准入 1024。这些是容量上限，不是建议把每个槽位都派出去——管理者应使用最小的有效扇出，保持单一汇总负责人，并在汇报整体完成前验证 worker 收据。"
            : "The sub-agent capacity source of truth is crates/tui/src/config/subagent_limits.rs: default configured concurrency is 64, maximum configured concurrency is 128, and maximum admitted running-plus-queued work is 1024. These are capacity ceilings, not advice to dispatch every slot — a manager should use the smallest useful fan-out, keep a single fan-in owner, and verify worker receipts before reporting combined completion."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/SUBAGENTS.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/SUBAGENTS.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
