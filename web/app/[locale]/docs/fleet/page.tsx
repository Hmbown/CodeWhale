import { PRODUCT_TERMS } from "@/lib/content/vocabulary";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/fleet",
    locale,
    title: isZh ? "Fleet 与 Workflow · Codewhale 文档" : "Fleet & Workflow · Codewhale Docs",
    description: isZh
      ? "持久多 worker 运行的本地控制平面，以及可选的 Workflow 编排层。"
      : "The control plane for durable multi-worker runs, plus the optional Workflow orchestration overlay.",
  });
}

export default async function FleetPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const vocabulary = PRODUCT_TERMS.map((row) => ({
    term: row.term,
    definition: isZh ? row.long.zh : row.long.en,
  }));

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "Fleet 与 Workflow" : "Fleet & Workflow"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Fleet 是面向持久多 worker 运行的控制平面。它不是独立的执行引擎：一个 Fleet worker 就是一次由 Fleet 启动并持久跟踪的 codewhale exec 无头运行。当工作需要重试、睡眠/重启后存活、远程执行、收据或可审计的台账时，使用 Fleet 而不是短寿命的 agent 扇出。"
            : "Fleet is the control plane for durable multi-worker runs. It is not a separate execution engine: a fleet worker is a headless codewhale exec run that the fleet launches and tracks durably. Reach for Fleet instead of short-lived agent fan-out whenever the work needs retry, sleep/restart survival, remote execution, receipts, or a ledgered audit trail."}
        </p>
        <div className="hairline-t mt-6">
          {vocabulary.map((row) => (
            <section key={row.term} className="py-4 hairline-b">
              <h3 className="font-display text-xl">{row.term}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.definition}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="cli" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "运行一次 Fleet" : "Run a fleet"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Fleet 状态存放在工作区的 .codewhale/fleet.jsonl 台账中，worker 日志在 .codewhale/fleet/ 下。codewhale fleet resume <run-id> 是重启恢复命令：它重放台账、调和停止心跳的在途租约，且幂等——在管理进程退出、笔记本睡眠或运行时重启后都可以安全运行。"
            : "Fleet state lives in the workspace's .codewhale/fleet.jsonl ledger, with worker logs under .codewhale/fleet/. codewhale fleet resume <run-id> is the restart-recovery verb: it replays the ledger, reconciles in-flight leases whose workers stopped heartbeating, and is idempotent — safe after a manager exit, laptop sleep, or runtime restart."}
        </p>
        <pre className="code-block mt-4">{`codewhale fleet init
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              注意两个同名状态面：TUI 里的 <code className="inline">/fleet status</code>（或{" "}
              <code className="inline">/subagents</code>）只显示当前交互会话的子 Agent；shell 里的{" "}
              <code className="inline">codewhale fleet status</code> 才读取持久 Fleet 台账。
            </>
          ) : (
            <>
              Two similarly named status surfaces exist: in the TUI,{" "}
              <code className="inline">/fleet status</code> (or <code className="inline">/subagents</code>)
              shows the sub-agents attached to the current interactive session; in a shell,{" "}
              <code className="inline">codewhale fleet status</code> reads the durable Fleet ledger.
            </>
          )}
        </p>
      </section>

      <section id="profiles" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "角色与 /fleet setup" : "Roles and /fleet setup"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "/fleet setup 打开一个渐进式向导，编写可复用的 agent 团队档案：一次只做一个选择——角色，然后是模型（可继承，或任何已配置提供商的具体模型），再是思考档位（inherit、off、low、medium、high、max 或 auto）——最后在审查页确认完整姿态（路由、思考、权限、工具、范围与审查策略）。档案可以写在项目级（.codewhale/agents/<role>.toml，随仓库走）或个人级（$CODEWHALE_HOME/agents/<role>.toml，本机所有仓库可用）；同名项目档案优先。档案的存储范围不会扩大运行操作的权限。"
            : "/fleet setup opens a progressive wizard for authoring a reusable agent-team profile: one focused choice at a time — a role, then a model (inherit, or a concrete model from any configured provider), then a thinking tier (inherit, off, low, medium, high, max, or auto) — and a review of the full posture (route, thinking, permissions, tools, scope, and review policy) before anything is saved. Profiles live in project scope (.codewhale/agents/<role>.toml, travels with the repo) or personal scope ($CODEWHALE_HOME/agents/<role>.toml, available in every repo on the machine); a same-id project profile wins. Profile storage scope never widens the authority of a running operation."}
        </p>
      </section>

      <section id="workflow" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "Workflow 编排" : "Workflow orchestration"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "普通多 Agent 工作不需要 Workflow：在 Operate 里直接发消息，需要并行、隔离或长时间工作时让 Codewhale 优先委派后台 worker 即可。只有当工作需要有序阶段、门禁、共享预算、回放或确定性汇总时才用 Workflow。Workflow 脚本是纯协调者：没有自己的文件系统和 shell，真正的工作由它启动的子 Agent 完成。脚本以编译专用的声明式 JS 子集编写，降低到类型化的 WorkflowSpec 后由 Rust 校验与执行；import、fetch、process、eval、async/await 等会产生副作用的写法会被编译器拒绝。"
            : "Ordinary multi-agent work does not need Workflow: send normal messages in Operate and let Codewhale prefer background workers when parallelism, isolation, or duration makes delegation useful. Use Workflow when ordered phases, gates, shared budgets, replay, or deterministic fan-in matter. A Workflow script is a coordinator only: it has no filesystem or shell of its own; real work happens in the sub-agents it launches. Scripts are written in a declarative compile-only JS subset that lowers to a typed WorkflowSpec validated and executed by Rust; effectful constructs such as import, fetch, process, eval, and async/await are rejected by the compiler."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "默认校验边界：每次 Workflow 运行最多 100 个 worker Agent、最多 5 层递归 Fleet 环、循环必须声明 max_iterations、动态 expand 节点必须声明 max_children 和模板。这些是数量上限而非并发要求——一个合法的 100 Agent Workflow 仍会按配置好的 Fleet worker 池排水执行。Workflow JS 沙箱内单 run 最多 16 个并发存活 Agent、整个 VM 生命周期最多 1,000 次启动。"
            : "Default validation bounds: up to 100 worker agents per workflow run, up to 5 recursive Fleet rings, loops must declare max_iterations, and dynamic expand nodes must declare max_children plus a template. These are population limits, not launch concurrency — a valid 100-agent workflow still drains through the configured Fleet worker pool. Inside the Workflow JS sandbox, one run keeps at most 16 concurrent live agents and at most 1,000 spawns over the VM lifetime."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/FLEET.md, docs/WORKFLOW_AUTHORING.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/FLEET.md, docs/WORKFLOW_AUTHORING.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
