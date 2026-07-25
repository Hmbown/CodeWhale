import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/hooks",
    locale,
    title: isZh ? "钩子 · Codewhale 文档" : "Hooks · Codewhale Docs",
    description: isZh
      ? "已发布的生命周期钩子：可变 message_submit、tool_call_before 决策、turn_end 与子 Agent 观察事件。"
      : "The shipped lifecycle hooks: mutable message_submit, tool_call_before decisions, turn_end, and sub-agent observer events.",
  });
}

export default async function HooksPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const events = isZh
    ? [
        {
          name: "message_submit（可变）",
          detail:
            "在用户消息进入历史或发给模型之前运行。钩子从 stdin 收到 JSON；exit 0 且 stdout 打印含非空 text 字段的 JSON 时替换提交文本；exit 2 在回合开始前阻止提交。多个钩子按配置顺序串行执行，每个钩子收到上一个钩子的输出文本。标记 background = true 的钩子只能观察，不能改写或阻止。",
        },
        {
          name: "tool_call_before（决策）",
          detail:
            "在每次工具调用执行前运行。除 exit 2 硬拒绝（始终生效）外，前台钩子可在 exit 0 时用 stdout JSON 给出决策：allow / deny / ask，并可附带 updatedInput 改写工具输入、additionalContext 追加进给模型的工具结果。多个钩子命中时优先级为 deny > ask > allow；tool_name 条件支持 * 通配（如 mcp__* 匹配所有 MCP 工具）。Full Access 不打开工具审批提示，因此 ask 不会降低该姿态。",
        },
        {
          name: "turn_end（观察）",
          detail:
            "在每个模型回合结束后触发，此时用量、成本、通知、收据和队列恢复状态都已更新。stdin 收到包含 status、duration_ms、usage、totals、queued_message_count 等字段的 JSON。stdout 被忽略，失败只记警告——不能阻止输入、改写 transcript 或改变下一个排队消息。",
        },
        {
          name: "subagent_spawn / subagent_complete（观察）",
          detail:
            "观察子 Agent 的启动与完成，stdin 收到有界的 JSON 元数据（agent_id、状态、截断后的 prompt/result 预览）。失败只记警告，不阻塞调度、不改 prompt 或结果；需要完整细节时使用 agent 返回的 transcript 句柄。",
        },
      ]
    : [
        {
          name: "message_submit (mutable)",
          detail:
            "Runs before a submitted message is added to history or sent to the model. The hook receives JSON on stdin; exit 0 with stdout JSON carrying a non-empty text field replaces the submitted text, and exit 2 blocks the submission before the turn starts. Multiple hooks run serially in config order, each receiving the previous hook's output. Hooks marked background = true are observer-only and cannot transform or block.",
        },
        {
          name: "tool_call_before (decision)",
          detail:
            "Runs before each tool call executes. Beyond the exit-2 hard deny (which always wins), a foreground hook may print a JSON decision on stdout with exit 0: allow / deny / ask, plus updatedInput to rewrite the tool input and additionalContext appended to the tool result the model sees. When several hooks match, precedence is deny > ask > allow; tool_name conditions support * globs (mcp__* matches every MCP tool). Full Access does not open tool-approval prompts, so ask does not downgrade that posture.",
        },
        {
          name: "turn_end (observer)",
          detail:
            "Fires after each model turn ends, once usage, cost, notifications, receipts, and queue-recovery state have settled. The stdin JSON carries fields such as status, duration_ms, usage, totals, and queued_message_count. Stdout is ignored and failures are warn-only — the hook cannot block input, mutate the transcript, or change the next queued follow-up.",
        },
        {
          name: "subagent_spawn / subagent_complete (observer)",
          detail:
            "Observe sub-agent start and completion with bounded JSON metadata on stdin (agent_id, status, truncated prompt/result previews). Failures are warn-only and never block scheduling or change prompts or results; use the transcript handle returned by agent when full detail is needed.",
        },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "钩子" : "Hooks"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "钩子让你把自己的命令挂进 Codewhale 的生命周期：在消息提交前注入上下文、在工具调用前执行策略、在回合结束或子 Agent 启停时做审计。本页描述当前已发布的行为；docs/rfcs/1364-hooks-lifecycle.md 是这组能力的设计 RFC，完整配置 schema 见 docs/CONFIGURATION.md。"
            : "Hooks attach your own commands to Codewhale's lifecycle: inject context before a message is submitted, enforce policy before a tool call, and audit turns or sub-agent activity. This page describes what currently ships; docs/rfcs/1364-hooks-lifecycle.md is the design RFC for this surface, and docs/CONFIGURATION.md carries the full configuration schema."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              钩子配置在 config.toml 的 <code className="inline">[[hooks.hooks]]</code> 条目下；TUI 里运行{" "}
              <code className="inline">/hooks</code> 可以按事件分组查看每个钩子的名称、命令预览、超时和条件，以及{" "}
              <code className="inline">[hooks].enabled</code> 的全局开关状态。
            </>
          ) : (
            <>
              Hooks are configured under <code className="inline">[[hooks.hooks]]</code> entries in
              config.toml; run <code className="inline">/hooks</code> in the TUI to see every configured
              hook grouped by event — name, command preview, timeout, and condition — plus the global{" "}
              <code className="inline">[hooks].enabled</code> state.
            </>
          )}
        </p>
        <div className="hairline-t mt-6">
          {events.map((row) => (
            <section key={row.name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{row.name}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="project" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "项目级钩子" : "Project-local hooks"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "仓库可以在 <workspace>/.codewhale/hooks.toml 中携带策略。因为项目钩子是可执行的 shell 配置，Codewhale 只有在工作区通过信任提示或用户配置中的 trust_level = \"trusted\" 被信任后才加载它们——会话内的 /trust on 和旧版 .deepseek/trusted 标记都不会单独启用项目钩子。受信任后，项目钩子追加在 config.toml 的全局钩子之后运行，因此对 updatedInput 而言最后生效。格式错误但已受信任的项目文件会记警告并回退到只用全局钩子。"
            : "Repositories can ship policy in <workspace>/.codewhale/hooks.toml. Because project hooks are executable shell configuration, Codewhale loads them only after the workspace is trusted through the trust prompt or a trust_level = \"trusted\" entry in user-owned config — session /trust on and legacy .deepseek/trusted markers do not enable project hooks by themselves. Once trusted, project hooks are appended after the global hooks from config.toml, so they run last and win updatedInput ties. A malformed trusted project file logs a warning and startup falls back to global hooks only."}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/rfcs/1364-hooks-lifecycle.md（设计 RFC）, docs/CONFIGURATION.md（配置 schema）· 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/rfcs/1364-hooks-lifecycle.md (design RFC), docs/CONFIGURATION.md (configuration schema) · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
