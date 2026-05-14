import Link from "next/link";
import { Seal } from "@/components/seal";
import { getFacts } from "@/lib/facts";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "文档 · DeepSeek TUI" : "Docs · DeepSeek TUI",
    description: isZh
      ? "DeepSeek TUI 的工作原理：模式、工具、沙箱、MCP、配置、钩子。"
      : "How DeepSeek TUI works: modes, tools, sandbox, MCP, config, hooks.",
  };
}

const sectionsEn = [
  { id: "modes", label: "Modes" },
  { id: "tools", label: "Tools" },
  { id: "approval", label: "Approval & Sandbox" },
  { id: "config", label: "Configuration" },
  { id: "mcp", label: "MCP" },
  { id: "skills", label: "Skills" },
  { id: "providers", label: "Providers" },
  { id: "shortcuts", label: "Shortcuts" },
];

const sectionsZh = [
  { id: "modes", label: "模式" },
  { id: "tools", label: "工具" },
  { id: "approval", label: "审批与沙箱" },
  { id: "config", label: "配置" },
  { id: "mcp", label: "MCP" },
  { id: "skills", label: "技能" },
  { id: "providers", label: "提供商" },
  { id: "shortcuts", label: "快捷键" },
];

export default async function DocsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const sections = isZh ? sectionsZh : sectionsEn;
  const facts = await getFacts();

  return (
    <>
      {isZh ? (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="文" />
              <div className="eyebrow">Section 02 · 文档</div>
            </div>
            <h1 className="font-display tracking-crisp">
              文档 <span className="font-cjk text-indigo text-5xl ml-2">Documentation</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
              工作原理简述。完整的架构讲解请参阅仓库中的
              <Link href="https://github.com/Hmbown/deepseek-tui/blob/main/docs/ARCHITECTURE.md" className="body-link mx-1">docs/ARCHITECTURE.md</Link>。
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-16 grid lg:grid-cols-12 gap-10 min-w-0">
            <aside className="lg:col-span-3 min-w-0">
              <div className="lg:sticky lg:top-32">
                <div className="eyebrow mb-3">本页目录 · On this page</div>
                <ul className="space-y-1.5 hairline-t hairline-b py-3">
                  {sections.map((s) => (
                    <li key={s.id}>
                      <a href={`#${s.id}`} className="text-sm hover:text-indigo block py-0.5">
                        <span className="font-mono text-[0.7rem] text-ink-mute mr-2 tabular">§</span>
                        {s.label}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            </aside>

            <article className="lg:col-span-9 space-y-14 min-w-0">

              {/* 模式 */}
              <section id="modes" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  模式 <span className="font-cjk text-indigo text-2xl ml-2">Modes</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  三种运行模式——与审批系统正交。按{" "}
                  <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">Tab</kbd> 切换。
                </p>
                <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b mt-6">
                  {[
                    { name: "Plan", cn: "计划", color: "text-cobalt", desc: "只读调查。可以 grep、读文件、列目录、抓取 URL——不能写入或执行 shell。" },
                    { name: "Agent", cn: "代理", color: "text-jade", desc: "默认模式。多步工具调用。Shell 和有副作用的工具需按 approval_mode 设置审批。" },
                    { name: "YOLO", cn: "全权", color: "text-indigo", desc: "自动批准所有操作并启用信任模式。工作区边界解除。请谨慎使用。" },
                  ].map((m) => (
                    <div key={m.name} className="p-5">
                      <div className={`font-display text-xl ${m.color} mb-1`}>
                        {m.name} <span className="font-cjk text-base ml-1.5">{m.cn}</span>
                      </div>
                      <p className="text-sm text-ink-soft leading-[1.9] tracking-wide">{m.desc}</p>
                    </div>
                  ))}
                </div>
              </section>

              {/* 工具 */}
              <section id="tools" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  工具 <span className="font-cjk text-indigo text-2xl ml-2">Tools</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  精选工具集——设计思路详见 <code className="inline">docs/TOOL_SURFACE.md</code>。
                </p>
                <div className="hairline-t hairline-b mt-6">
                  {[
                    { group: "文件操作", tools: "read_file · list_dir · write_file · edit_file · apply_patch" },
                    { group: "搜索", tools: "grep_files · file_search · web_search · fetch_url" },
                    { group: "Shell", tools: "exec_shell · exec_shell_wait · exec_shell_interact" },
                    { group: "Git / 诊断 / 测试", tools: "git_status · git_diff · diagnostics · run_tests" },
                    { group: "子 Agent", tools: "agent_open · agent_eval · agent_close —— 持久会话，并行执行，通过 var_handle 读取大结果" },
                    { group: "递归 LM (RLM)", tools: "rlm_open · rlm_eval · rlm_configure · rlm_close —— 沙箱 Python REPL，内置 peek/search/chunk/sub_query_batch 等辅助函数" },
                    { group: "MCP", tools: "mcp_<server>_<tool>——从 ~/.deepseek/mcp.json 自动注册" },
                  ].map((row) => (
                    <div key={row.group} className="grid md:grid-cols-12 gap-0 hairline-t py-3 px-4 hover:bg-paper-deep transition-colors min-w-0">
                      <div className="md:col-span-3 font-display text-sm font-semibold">{row.group}</div>
                      <div className="md:col-span-9 font-mono text-[0.78rem] text-ink-soft leading-relaxed break-words min-w-0">{row.tools}</div>
                    </div>
                  ))}
                </div>
              </section>

              {/* 审批 */}
              <section id="approval" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  审批与沙箱 <span className="font-cjk text-indigo text-2xl ml-2">Approval</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  模式与审批是两个独立的维度。通过 <code className="inline">/config</code> 设置。
                </p>
                <div className="hairline-t hairline-b mt-6 grid md:grid-cols-3 col-rule">
                  {[
                    { name: "suggest", cn: "建议", desc: "默认——按模式规则执行。危险操作前询问。" },
                    { name: "auto", cn: "自动", desc: "自动批准所有工具调用。等同于无信任的 YOLO。" },
                    { name: "never", cn: "拒绝", desc: "阻止任何非安全/非只读操作。仅限调查。" },
                  ].map((a) => (
                    <div key={a.name} className="p-5">
                      <div className="font-mono text-sm text-indigo uppercase tracking-wider">{a.name} · <span className="font-cjk normal-case tracking-normal">{a.cn}</span></div>
                      <p className="text-sm text-ink-soft mt-2 leading-[1.9] tracking-wide">{a.desc}</p>
                    </div>
                  ))}
                </div>
                <p className="mt-5 text-ink-soft leading-[1.9] tracking-wide">
                  沙箱：{facts.sandboxBackends.join("、")}。工作区边界默认为 <code className="inline">--workspace</code>。
                  <code className="inline">/trust</code> 可解除边界限制。
                </p>
              </section>

              {/* 配置 */}
              <section id="config" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  配置 <span className="font-cjk text-indigo text-2xl ml-2">Configuration</span>
                </h2>
                <pre className="code-block mt-5">
{`# ~/.deepseek/config.toml
api_key = "sk-..."
base_url = "https://api.deepseek.com"
default_text_model = "${facts.defaultModel ?? "deepseek-v4-pro"}"  # 默认模型；deepseek-v4-flash 用于快速 / 子智能体

[ui]
default_mode = "agent"                      # plan | agent | yolo
approval_mode = "suggest"                   # suggest | auto | never
reasoning_effort = "high"                   # off | high | max

[hooks]
enabled = true
default_timeout_secs = 30

[[hooks.hooks]]
event = "session_start"                     # 也支持: tool_call_before / tool_call_after
command = "~/.deepseek/hooks/pre.sh"        # / message_submit / mode_change / on_error / shell_env`}
                </pre>
                <p className="mt-4 text-sm text-ink-soft">
                  完整参考：<Link className="body-link" href="https://github.com/Hmbown/deepseek-tui/blob/main/config.example.toml">config.example.toml</Link>。
                </p>
              </section>

              {/* MCP */}
              <section id="mcp" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  MCP 服务器 <span className="font-cjk text-indigo text-2xl ml-2">MCP</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  <code className="inline">deepseek</code> 双向支持模型上下文协议（Model Context Protocol）：作为客户端从
                  <code className="inline">~/.deepseek/mcp.json</code> 加载服务器，同时也可作为服务器暴露工具
                  （<code className="inline">deepseek mcp</code>）。工具以 <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code> 形式呈现。
                </p>
                <pre className="code-block mt-5">
{`{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me"]
    },
    "sqlite": {
      "command": "uvx",
      "args": ["mcp-server-sqlite", "--db-path", "./data.db"]
    }
  }
}`}
                </pre>
              </section>

              {/* 技能 */}
              <section id="skills" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  技能 <span className="font-cjk text-indigo text-2xl ml-2">Skills</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  技能是 <code className="inline">~/.deepseek/skills/&lt;name&gt;/</code> 下的一个文件夹，
                  根目录包含 <code className="inline">SKILL.md</code>。Agent 启动时加载技能名称和描述，
                  在需要时通过 Skill 工具拉取完整内容。
                </p>
              </section>

              {/* 提供商 */}
              <section id="providers" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  提供商 <span className="font-cjk text-indigo text-2xl ml-2">Providers</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-[1.9] tracking-wide">
                  使用 <code className="inline">deepseek auth set --provider &lt;id&gt;</code> 切换。下表为
                  <code className="inline">crates/tui/src/config.rs</code> 中 <code className="inline">ApiProvider</code> 枚举的实时投影
                  ，目前共 {facts.providers.length} 个。
                </p>
                <div className="hairline-t hairline-b mt-5">
                  {facts.providers.map((p) => (
                    <div key={p.id} className="grid md:grid-cols-12 gap-0 hairline-t py-3 px-4 hover:bg-paper-deep min-w-0">
                      <div className="md:col-span-3 font-display font-semibold">{p.label}</div>
                      <div className="md:col-span-3 font-mono text-[0.78rem] text-ink-soft break-words min-w-0">{p.id}</div>
                      <div className="md:col-span-6 font-mono text-[0.78rem] text-ink-soft break-words min-w-0">{p.env}</div>
                    </div>
                  ))}
                </div>
              </section>

              {/* 快捷键 */}
              <section id="shortcuts" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  快捷键 <span className="font-cjk text-indigo text-2xl ml-2">Shortcuts</span>
                </h2>
                <div className="hairline-t hairline-b mt-5 grid md:grid-cols-2 col-rule">
                  {[
                    { k: "Tab", v: "切换模式（Plan / Agent / YOLO）" },
                    { k: "Shift+Tab", v: "切换推理强度" },
                    { k: "Ctrl+L", v: "清屏，保留会话" },
                    { k: "Ctrl+C", v: "取消当前轮次" },
                    { k: "Ctrl+D", v: "退出" },
                    { k: "/help", v: "斜杠命令面板" },
                    { k: "/config", v: "交互式编辑配置" },
                    { k: "/trust", v: "解除本会话的工作区边界" },
                  ].map((s) => (
                    <div key={s.k} className="p-4 flex items-center gap-4 hairline-t">
                      <kbd className="font-mono text-xs px-2 py-1 hairline-t hairline-b hairline-l hairline-r bg-paper-deep min-w-[5rem] text-center">{s.k}</kbd>
                      <span className="text-sm text-ink-soft">{s.v}</span>
                    </div>
                  ))}
                </div>
              </section>

            </article>
          </section>
        </>
      ) : (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="文" />
              <div className="eyebrow">Section 02 · 文档</div>
            </div>
            <h1 className="font-display tracking-crisp">
              Documentation <span className="font-cjk text-indigo text-5xl ml-2">文档</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-relaxed">
              The short version of how it works. For the full architecture walk-through, see
              <Link href="https://github.com/Hmbown/deepseek-tui/blob/main/docs/ARCHITECTURE.md" className="body-link mx-1">docs/ARCHITECTURE.md</Link>
              in the repo.
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-16 grid lg:grid-cols-12 gap-10 min-w-0">
            <aside className="lg:col-span-3 min-w-0">
              <div className="lg:sticky lg:top-32">
                <div className="eyebrow mb-3">On this page · 目录</div>
                <ul className="space-y-1.5 hairline-t hairline-b py-3">
                  {sections.map((s) => (
                    <li key={s.id}>
                      <a href={`#${s.id}`} className="text-sm hover:text-indigo block py-0.5">
                        <span className="font-mono text-[0.7rem] text-ink-mute mr-2 tabular">§</span>
                        {s.label}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            </aside>

            <article className="lg:col-span-9 space-y-14 min-w-0">

              <section id="modes" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Modes <span className="font-cjk text-indigo text-2xl ml-2">模式</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  Three operating modes — orthogonal to the approval system. Cycle with{" "}
                  <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">Tab</kbd>.
                </p>
                <div className="grid md:grid-cols-3 gap-0 col-rule hairline-t hairline-b mt-6">
                  {[
                    { name: "Plan", cn: "计划", color: "text-cobalt", desc: "Read-only investigation. The agent can grep, read files, list dirs, fetch URLs — never write or shell out." },
                    { name: "Agent", cn: "代理", color: "text-jade", desc: "Default. Multi-step tool use. Shell and side-effectful tools require approval per `approval_mode` setting." },
                    { name: "YOLO", cn: "全权", color: "text-indigo", desc: "Auto-approve everything + enable trust mode. Workspace boundary lifts. Use carefully." },
                  ].map((m) => (
                    <div key={m.name} className="p-5">
                      <div className={`font-display text-xl ${m.color} mb-1`}>
                        {m.name} <span className="font-cjk text-base ml-1.5">{m.cn}</span>
                      </div>
                      <p className="text-sm text-ink-soft leading-relaxed">{m.desc}</p>
                    </div>
                  ))}
                </div>
              </section>

              <section id="tools" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Tools <span className="font-cjk text-indigo text-2xl ml-2">工具</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  Curated surface — see <code className="inline">docs/TOOL_SURFACE.md</code> for design rationale.
                </p>
                <div className="hairline-t hairline-b mt-6">
                  {[
                    { group: "File ops", tools: "read_file · list_dir · write_file · edit_file · apply_patch" },
                    { group: "Search", tools: "grep_files · file_search · web_search · fetch_url" },
                    { group: "Shell", tools: "exec_shell · exec_shell_wait · exec_shell_interact" },
                    { group: "Git / diag / test", tools: "git_status · git_diff · diagnostics · run_tests" },
                    { group: "Sub-agents", tools: "agent_open · agent_eval · agent_close — persistent sessions, parallel execution, bounded result retrieval via var_handle" },
                    { group: "Recursive LM (RLM)", tools: "rlm_open · rlm_eval · rlm_configure · rlm_close — sandboxed Python REPL with peek/search/chunk/sub_query_batch helpers" },
                    { group: "MCP", tools: "mcp_<server>_<tool> — auto-registered from ~/.deepseek/mcp.json" },
                  ].map((row) => (
                    <div key={row.group} className="grid md:grid-cols-12 gap-0 hairline-t py-3 px-4 hover:bg-paper-deep transition-colors min-w-0">
                      <div className="md:col-span-3 font-display text-sm font-semibold">{row.group}</div>
                      <div className="md:col-span-9 font-mono text-[0.78rem] text-ink-soft leading-relaxed break-words min-w-0">{row.tools}</div>
                    </div>
                  ))}
                </div>
              </section>

              <section id="approval" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Approval & Sandbox <span className="font-cjk text-indigo text-2xl ml-2">审批</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  Mode and approval are independent axes. Set via <code className="inline">/config</code>.
                </p>
                <div className="hairline-t hairline-b mt-6 grid md:grid-cols-3 col-rule">
                  {[
                    { name: "suggest", cn: "建议", desc: "Default — uses per-mode rules. Asks before risky ops." },
                    { name: "auto", cn: "自动", desc: "Auto-approve all tool calls. Equivalent to YOLO without trust." },
                    { name: "never", cn: "拒绝", desc: "Block anything not safe/read-only. Investigation only." },
                  ].map((a) => (
                    <div key={a.name} className="p-5">
                      <div className="font-mono text-sm text-indigo uppercase tracking-wider">{a.name} · <span className="font-cjk normal-case tracking-normal">{a.cn}</span></div>
                      <p className="text-sm text-ink-soft mt-2 leading-relaxed">{a.desc}</p>
                    </div>
                  ))}
                </div>
                <p className="mt-5 text-ink-soft leading-relaxed">
                  Sandbox: {facts.sandboxBackends.join(", ")}. Workspace boundary defaults to{" "}
                  <code className="inline">--workspace</code>. <code className="inline">/trust</code> lifts the boundary.
                </p>
              </section>

              <section id="config" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Configuration <span className="font-cjk text-indigo text-2xl ml-2">配置</span>
                </h2>
                <pre className="code-block mt-5">
{`# ~/.deepseek/config.toml
api_key = "sk-..."
base_url = "https://api.deepseek.com"
default_text_model = "${facts.defaultModel ?? "deepseek-v4-pro"}"  # default; deepseek-v4-flash is the fast / sub-agent option

[ui]
default_mode = "agent"                      # plan | agent | yolo
approval_mode = "suggest"                   # suggest | auto | never
reasoning_effort = "high"                   # off | high | max

[hooks]
enabled = true
default_timeout_secs = 30

[[hooks.hooks]]
event = "session_start"                     # or: tool_call_before / tool_call_after
command = "~/.deepseek/hooks/pre.sh"        # / message_submit / mode_change / on_error / shell_env`}
                </pre>
                <p className="mt-4 text-sm text-ink-soft">
                  Full reference: <Link className="body-link" href="https://github.com/Hmbown/deepseek-tui/blob/main/config.example.toml">config.example.toml</Link>.
                </p>
              </section>

              <section id="mcp" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  MCP Servers <span className="font-cjk text-indigo text-2xl ml-2">MCP</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  <code className="inline">deepseek</code> speaks the Model Context Protocol both ways: as a client (loads
                  servers from <code className="inline">~/.deepseek/mcp.json</code>) and as a server
                  (<code className="inline">deepseek mcp</code>). Tools surface as <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code>.
                </p>
                <pre className="code-block mt-5">
{`{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me"]
    },
    "sqlite": {
      "command": "uvx",
      "args": ["mcp-server-sqlite", "--db-path", "./data.db"]
    }
  }
}`}
                </pre>
              </section>

              <section id="skills" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Skills <span className="font-cjk text-indigo text-2xl ml-2">技能</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  A skill is a folder under <code className="inline">~/.deepseek/skills/&lt;name&gt;/</code>
                  with a <code className="inline">SKILL.md</code> at the root. The agent loads skill names + descriptions on
                  startup and can pull in the full body via the Skill tool when relevant.
                </p>
              </section>

              <section id="providers" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Providers <span className="font-cjk text-indigo text-2xl ml-2">提供商</span>
                </h2>
                <p className="text-ink-soft mt-3 leading-relaxed">
                  Switch with <code className="inline">deepseek auth set --provider &lt;id&gt;</code>. The
                  table below is a live projection of the <code className="inline">ApiProvider</code> enum
                  in <code className="inline">crates/tui/src/config.rs</code> — currently {facts.providers.length} providers.
                </p>
                <div className="hairline-t hairline-b mt-5">
                  {facts.providers.map((p) => (
                    <div key={p.id} className="grid md:grid-cols-12 gap-0 hairline-t py-3 px-4 hover:bg-paper-deep min-w-0">
                      <div className="md:col-span-3 font-display font-semibold">{p.label}</div>
                      <div className="md:col-span-3 font-mono text-[0.78rem] text-ink-soft break-words min-w-0">{p.id}</div>
                      <div className="md:col-span-6 font-mono text-[0.78rem] text-ink-soft break-words min-w-0">{p.env}</div>
                    </div>
                  ))}
                </div>
              </section>

              <section id="shortcuts" className="scroll-mt-32">
                <h2 className="font-display text-3xl mb-1">
                  Shortcuts <span className="font-cjk text-indigo text-2xl ml-2">快捷键</span>
                </h2>
                <div className="hairline-t hairline-b mt-5 grid md:grid-cols-2 col-rule">
                  {[
                    { k: "Tab", v: "Cycle mode (Plan / Agent / YOLO)" },
                    { k: "Shift+Tab", v: "Cycle reasoning effort" },
                    { k: "Ctrl+L", v: "Clear screen, keep session" },
                    { k: "Ctrl+C", v: "Cancel current turn" },
                    { k: "Ctrl+D", v: "Exit" },
                    { k: "/help", v: "Slash command palette" },
                    { k: "/config", v: "Edit config interactively" },
                    { k: "/trust", v: "Lift workspace boundary for session" },
                  ].map((s) => (
                    <div key={s.k} className="p-4 flex items-center gap-4 hairline-t">
                      <kbd className="font-mono text-xs px-2 py-1 hairline-t hairline-b hairline-l hairline-r bg-paper-deep min-w-[5rem] text-center">{s.k}</kbd>
                      <span className="text-sm text-ink-soft">{s.v}</span>
                    </div>
                  ))}
                </div>
              </section>

            </article>
          </section>
        </>
      )}
    </>
  );
}
