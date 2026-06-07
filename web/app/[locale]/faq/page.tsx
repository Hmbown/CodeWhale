import Link from "next/link";
import { Seal } from "@/components/seal";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "常见问题 · CodeWhale" : "FAQ · CodeWhale",
    description: isZh
      ? "CodeWhale 常见问题：安装、配置、提供商、模型、模式、安全与隐私。答案来自实际代码、文档和 GitHub 议题。"
      : "CodeWhale frequently asked questions: install, config, providers, models, modes, security, and privacy. Answers sourced from real code, docs, and GitHub issues.",
  };
}

interface FaqItem {
  q: string;
  a: React.ReactNode;
  sources?: string[];
}

const faqEn: FaqItem[] = [
  {
    q: "What is CodeWhale?",
    a: (
      <>
        CodeWhale is a terminal-native coding agent for open-source and open-weight models. It runs from the <code className="inline">codewhale</code> command, streams reasoning blocks, edits local workspaces with approval gates, and can auto-route each turn to the right model and thinking level. DeepSeek V4 is the first-class model path; OpenRouter is ready. Hugging Face, self-hosted, and other open-model surfaces are on the roadmap.
      </>
    ),
    sources: ["README.md", "docs/ARCHITECTURE.md"],
  },
  {
    q: "How do I install CodeWhale?",
    a: (
      <>
        <p className="mb-2">Four paths, same result:</p>
        <pre className="code-block mb-2">
{`# npm (recommended — no Rust toolchain needed)
npm install -g codewhale

# Cargo (needs Rust 1.88+)
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked

# Homebrew (macOS)
brew tap Hmbown/deepseek-tui && brew install deepseek-tui

# Direct download
# https://github.com/Hmbown/CodeWhale/releases`}
        </pre>
        <p>
          Run <code className="inline">codewhale</code> to start. First run creates <code className="inline">~/.codewhale/</code> automatically. Legacy <code className="inline">~/.deepseek/</code> is still read as a compatibility fallback.
          See the <Link href="/install" className="body-link">full install guide</Link> for China mirrors, Docker, and troubleshooting.
        </p>
      </>
    ),
    sources: ["README.md", "#1860", "#1914"],
  },
  {
    q: "What's the difference between codewhale and codewhale-tui?",
    a: (
      <>
        <code className="inline">codewhale</code> is the dispatcher CLI — it manages config, auth, updates, and launches the TUI.
        <code className="inline">codewhale-tui</code> is the terminal UI binary that runs the agent loop.
        When you type <code className="inline">codewhale</code>, the dispatcher spawns <code className="inline">codewhale-tui</code> for you.
        Both are installed together; you rarely need to think about the split.
      </>
    ),
    sources: ["README.md"],
  },
  {
    q: "Is CodeWhale the same as DeepSeek TUI? What about the rename?",
    a: (
      <>
        Yes. CodeWhale is the new name for what was previously called DeepSeek TUI.
        The canonical command is now <code className="inline">codewhale</code>. Legacy <code className="inline">deepseek</code> and <code className="inline">deepseek-tui</code> commands remain as compatibility shims — they still work.
        Config lives at <code className="inline">~/.codewhale/</code>. Legacy <code className="inline">~/.deepseek/</code> config is still read as a compatibility fallback, and <code className="inline">DEEPSEEK_*</code> env vars continue to work.
        DeepSeek is not deprecated. The rename reflects CodeWhale's broader mission as the agentic terminal for open models across providers, not a narrowing away from DeepSeek.
      </>
    ),
    sources: ["docs/REBRAND.md", "README.md"],
  },
  {
    q: "How do I set my API key?",
    a: (
      <>
        <pre className="code-block mb-2">
{`# Method 1: Environment variable
export DEEPSEEK_API_KEY=sk-...

# Method 2: Saved config (recommended — survives shell restarts)
codewhale auth set --provider deepseek --api-key sk-...

# Method 3: config.toml
# Add to ~/.codewhale/config.toml:
api_key = "sk-..."

# Check what's active:
codewhale auth status    # shows config, keyring, and env-var state
codewhale doctor         # full connectivity check`}
        </pre>
        <p>
          Saved config keys take precedence over environment variables.
          Use <code className="inline">codewhale auth clear --provider deepseek</code> to remove a saved key.
        </p>
      </>
    ),
    sources: ["#907", "#1545", "docs/CONFIGURATION.md"],
  },
  {
    q: "Which providers does CodeWhale support?",
    a: (
      <>
        <p className="mb-2">CodeWhale ships with these built-in providers:</p>
        <ul className="list-disc pl-5 space-y-1 text-sm text-ink-soft mb-3">
          <li><strong>DeepSeek</strong> — first-class, native API. Reasoning streaming, cache metrics, thinking effort control.</li>
          <li><strong>OpenRouter</strong> — unified API for DeepSeek models and more.</li>
          <li><strong>OpenAI</strong>, <strong>NVIDIA NIM</strong>, <strong>Volcengine Ark</strong>, <strong>Xiaomi MiMo</strong>, <strong>SiliconFlow</strong>, <strong>Novita</strong>, <strong>Fireworks</strong>, <strong>sglang</strong>, <strong>vLLM</strong>, <strong>Ollama</strong></li>
        </ul>
        <p>
          Set the corresponding env var (e.g. <code className="inline">OPENROUTER_API_KEY</code>) and your provider in <code className="inline">~/.codewhale/config.toml</code>.
          Self-hosted OpenAI-compatible endpoints are supported through the provider config.
        </p>
      </>
    ),
    sources: ["docs/CONFIGURATION.md", "#1978", "#1710"],
  },
  {
    q: "How do I use OpenRouter with CodeWhale?",
    a: (
      <>
        <pre className="code-block mb-2">
{`# 1. Set your OpenRouter key
export OPENROUTER_API_KEY=sk-or-v1-...

# 2. In ~/.codewhale/config.toml:
[providers.openrouter]
api_key = "sk-or-v1-..."

# 3. Run with an OpenRouter model:
codewhale --model openrouter/deepseek/deepseek-v4-pro

# Or set it as default in config.toml:
default_text_model = "openrouter/deepseek/deepseek-v4-pro"`}
        </pre>
        <p>
          OpenRouter uses the same reasoning/cache parser as the native DeepSeek provider.
          Model IDs follow the <code className="inline">provider/model-id</code> pattern (e.g. <code className="inline">openrouter/deepseek/deepseek-v4-flash</code>).
        </p>
      </>
    ),
    sources: ["docs/CONFIGURATION.md", "#1978"],
  },
  {
    q: "Can I use self-hosted or local models (vLLM, Ollama, llama.cpp)?",
    a: (
      <>
        Yes. Use the <code className="inline">vllm</code>, <code className="inline">sglang</code>, or <code className="inline">ollama</code> providers with your local endpoint.
        For OpenAI-compatible endpoints (llama.cpp server, text-generation-webui, Aphrodite, etc.), you can use the <code className="inline">openai</code> provider with a custom <code className="inline">base_url</code>.
        CodeWhale also respects <code className="inline">DEEPSEEK_ALLOW_INSECURE_HTTP=true</code> for local HTTP endpoints.
        Full Hugging Face TGI/vLLM integration is on the roadmap.
      </>
    ),
    sources: ["#574", "#1303", "docs/CONFIGURATION.md"],
  },
  {
    q: "What are Plan, Agent, and YOLO modes?",
    a: (
      <>
        <ul className="list-disc pl-5 space-y-2 text-sm text-ink-soft">
          <li><strong>Plan</strong> — Read-only investigation. Can grep, read files, list directories, fetch URLs. Cannot write or execute shell.</li>
          <li><strong>Agent</strong> — Default mode. Multi-step tool calling. Shell and side-effect tools require approval based on your approval_mode setting.</li>
          <li><strong>YOLO</strong> — Auto-approves all operations and enables trust mode. Workspace boundaries lift. Use carefully.</li>
        </ul>
        <p className="mt-2">
          Press <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">Tab</kbd> to cycle modes.
          Approval mode (suggest / auto / never) is orthogonal — you can be in Agent mode with auto-approval, for example.
        </p>
      </>
    ),
    sources: ["docs/MODES.md"],
  },
  {
    q: "What is model auto-routing? What is Fin?",
    a: (
      <>
        <p className="mb-2">
          Use <code className="inline">codewhale --model auto</code> or <code className="inline">/model auto</code> to let CodeWhale decide how much model power each turn needs.
        </p>
        <p className="mb-2">
          <strong>Fin</strong> is the fast non-thinking path (<code className="inline">deepseek-v4-flash</code> with thinking off) used for routing decisions, summaries, RLM children, context maintenance, and other coordination work. Before the real turn is sent, Fin makes a small routing call to pick the concrete model and thinking level.
        </p>
        <p>
          Short/simple turns can stay on Flash with thinking off. Coding, debugging, release work, architecture, or security review can move up to Pro and/or higher thinking. Fin is local to CodeWhale — the upstream API never receives <code className="inline">model: "auto"</code>.
        </p>
      </>
    ),
    sources: ["README.md", "#1207"],
  },
  {
    q: "What does /goal do?",
    a: (
      <>
        <code className="inline">/goal</code> is a simple goal-setter for the current session.
        It does not add another app mode; the mode switcher remains Plan, Agent, and YOLO.
        Track progress in <a href="https://github.com/Hmbown/CodeWhale/issues/891" className="body-link">#891</a>.
      </>
    ),
    sources: ["#891"],
  },
  {
    q: "Is my code safe? What sandboxing does CodeWhale use?",
    a: (
      <>
        CodeWhale runs entirely on your machine. No telemetry, no cloud processing of your code.
        Sandbox backends: <strong>seatbelt</strong> (macOS), <strong>landlock</strong> (Linux), restricted tokens (Windows).
        Workspace boundaries default to <code className="inline">--workspace</code>. <code className="inline">/trust</code> lifts them.
        Approval mode is configurable per session. All credential/approval/elevation events are written to <code className="inline">~/.codewhale/audit.log</code>.
      </>
    ),
    sources: ["SECURITY.md", "docs/ARCHITECTURE.md"],
  },
  {
    q: "How do MCP servers work?",
    a: (
      <>
        CodeWhale is a bidirectional MCP client and server. Define servers in <code className="inline">~/.codewhale/mcp.json</code>.
        Tools appear as <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code>. You can also expose CodeWhale as an MCP server with <code className="inline">codewhale mcp</code>.
        See the <Link href="/docs#mcp" className="body-link">docs page</Link> for configuration examples.
      </>
    ),
    sources: ["docs/MCP.md"],
  },
  {
    q: "How do I contribute?",
    a: (
      <>
        No CLA required. Fork, branch with conventional commits (<code className="inline">feat:</code>, <code className="inline">fix:</code>, etc.), run the local checks, open a PR.
        The maintainer reads everything personally. Start with issues labeled <code className="inline">good first issue</code>.
        See the <Link href="/contribute" className="body-link">contribute page</Link> and <a href="https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md" className="body-link">CONTRIBUTING.md</a>.
      </>
    ),
    sources: ["CONTRIBUTING.md"],
  },
  {
    q: "I'm in China — how do I install? Downloads are slow.",
    a: (
      <>
        Use mirror registries:
        <pre className="code-block my-2">
{`# npm mirror
npm config set registry https://registry.npmmirror.com
npm install -g codewhale

# Cargo mirror (Tsinghua TUNA)
# Add to ~/.cargo/config.toml:
[source.crates-io]
replace-with = "tuna"
[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"`}
        </pre>
        <p>
          Prebuilt binaries are also available from <a href="https://github.com/Hmbown/CodeWhale/releases" className="body-link">GitHub Releases</a>.
          The Gitee mirror and CNB mirror may also be available.
        </p>
      </>
    ),
    sources: ["README.md", "#1914", "docs/CNB_MIRROR.md"],
  },
  {
    q: "My API key was rejected or I get auth errors on first run.",
    a: (
      <>
        <p className="mb-2">Run <code className="inline">codewhale doctor</code> — it checks API key, network, sandbox, and MCP servers. Full report is written to <code className="inline">~/.codewhale/doctor.log</code>.</p>
        <p className="mb-2">Common causes:</p>
        <ul className="list-disc pl-5 space-y-1 text-sm text-ink-soft">
          <li>Stale <code className="inline">DEEPSEEK_API_KEY</code> in shell startup file — open a fresh shell or use <code className="inline">codewhale auth set</code></li>
          <li>Key from wrong provider — make sure the key matches the provider you're using</li>
          <li>Network connectivity — check <code className="inline">curl https://api.deepseek.com/v1/models</code></li>
        </ul>
      </>
    ),
    sources: ["#907", "#1545"],
  },
  {
    q: "What is Model Lab? When will Hugging Face integration be available?",
    a: (
      <>
        Model Lab is the planned open-model infrastructure layer: Hugging Face Hub API for model discovery, model cards, datasets, safetensors adapters, inference providers, and Jobs.
        It is NOT fully implemented. Track progress in <a href="https://github.com/Hmbown/CodeWhale/issues/1977" className="body-link">#1977</a>.
        Currently, you can use Hugging Face models through the OpenRouter provider or self-hosted endpoints.
      </>
    ),
    sources: ["#1977", "docs/MODEL_LAB.md"],
  },
  {
    q: "Why is token consumption so high? / Why is cache hit rate low?",
    a: (
      <>
        CodeWhale sends substantial context (system prompt, project instructions, tool definitions) with each turn.
        DeepSeek's prefix cache is used aggressively — the system prompt is layered to maximize cache hits.
        If you see high token usage, check: are you using <code className="inline">deepseek-v4-pro</code> for simple queries better suited to Flash?
        Model auto-routing (Fin) can help pick the right model per turn.
        Cache hit rate depends on prompt stability — modifying the system prompt or switching models resets the cache.
      </>
    ),
    sources: ["#1177", "#1818", "#743"],
  },
  {
    q: "How do I update CodeWhale?",
    a: (
      <>
        <pre className="code-block mb-2">
{`# Release-binary updater (works for npm/release-binary installs)
codewhale update

# npm
npm install -g codewhale@latest

# Cargo
cargo install codewhale-cli --locked --force

# Homebrew
brew update && brew upgrade deepseek-tui`}
        </pre>
        <p>
          If you installed via npm, <code className="inline">codewhale update</code> downloads the latest release binaries.
          If a mirror is lagging, download directly from <a href="https://github.com/Hmbown/CodeWhale/releases" className="body-link">GitHub Releases</a>.
        </p>
      </>
    ),
    sources: ["README.md", "#1869", "#1914"],
  },
];

const faqZh: FaqItem[] = [
  {
    q: "CodeWhale 是什么？",
    a: (
      <>
        CodeWhale 是一个面向开源模型的终端原生编程智能体。通过 <code className="inline">codewhale</code> 命令启动，流式输出推理块，在有审批门槛的情况下编辑本地工作区，并可为每个回合自动选择最合适的模型和推理深度。DeepSeek V4 是一级模型路径；OpenRouter 已就绪。Hugging Face、自托管等开放模型接口已在路线图中。
      </>
    ),
    sources: ["README.md", "docs/ARCHITECTURE.md"],
  },
  {
    q: "如何安装 CodeWhale？",
    a: (
      <>
        <p className="mb-2">四种方式，殊途同归：</p>
        <pre className="code-block mb-2">
{`# npm（推荐 — 无需 Rust 工具链）
npm install -g codewhale

# Cargo（需要 Rust 1.88+）
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked

# Homebrew（macOS）
brew tap Hmbown/deepseek-tui && brew install deepseek-tui

# 直接下载
# https://github.com/Hmbown/CodeWhale/releases`}
        </pre>
        <p>
          输入 <code className="inline">codewhale</code> 即可启动。首次运行会自动创建 <code className="inline">~/.codewhale/</code>。旧版 <code className="inline">~/.deepseek/</code> 仍会作为兼容回退读取。
          查看 <Link href="/zh/install" className="body-link">完整安装指南</Link> 了解国内镜像、Docker 和故障排除。
        </p>
      </>
    ),
    sources: ["README.md", "#1860", "#1914"],
  },
  {
    q: "codewhale 和 codewhale-tui 有什么区别？",
    a: (
      <>
        <code className="inline">codewhale</code> 是调度 CLI——管理配置、认证、更新，并启动 TUI。
        <code className="inline">codewhale-tui</code> 是运行智能体循环的终端 UI 二进制文件。
        当你输入 <code className="inline">codewhale</code> 时，调度器会自动为你启动 <code className="inline">codewhale-tui</code>。
        两者同时安装；通常你不需要关心这个区别。
      </>
    ),
    sources: ["README.md"],
  },
  {
    q: "CodeWhale 和 DeepSeek TUI 是什么关系？改名是怎么回事？",
    a: (
      <>
        CodeWhale 是 DeepSeek TUI 的新名称。当前的主命令是 <code className="inline">codewhale</code>。旧的 <code className="inline">deepseek</code> 和 <code className="inline">deepseek-tui</code> 命令作为兼容垫片继续有效。
        配置存放在 <code className="inline">~/.codewhale/</code>。旧版 <code className="inline">~/.deepseek/</code> 配置仍会作为兼容回退读取，<code className="inline">DEEPSEEK_*</code> 环境变量继续有效。
        DeepSeek 并未被弃用。改名是为了体现 CodeWhale 更广泛的使命——成为面向所有提供商的开放模型智能体终端，而非弱化 DeepSeek 的地位。
      </>
    ),
    sources: ["docs/REBRAND.md", "README.md"],
  },
  {
    q: "如何设置 API 密钥？",
    a: (
      <>
        <pre className="code-block mb-2">
{`# 方法 1：环境变量
export DEEPSEEK_API_KEY=sk-...

# 方法 2：保存在配置中（推荐 — 重启 Shell 后仍然有效）
codewhale auth set --provider deepseek --api-key sk-...

# 方法 3：config.toml
# 在 ~/.codewhale/config.toml 中添加：
api_key = "sk-..."

# 查看当前状态：
codewhale auth status    # 显示配置、密钥环和环境变量状态
codewhale doctor         # 完整连接检查`}
        </pre>
        <p>
          配置中保存的密钥优先于环境变量。
          使用 <code className="inline">codewhale auth clear --provider deepseek</code> 移除已保存的密钥。
        </p>
      </>
    ),
    sources: ["#907", "#1545", "docs/CONFIGURATION.md"],
  },
  {
    q: "CodeWhale 支持哪些提供商？",
    a: (
      <>
        <p className="mb-2">CodeWhale 内建以下提供商：</p>
        <ul className="list-disc pl-5 space-y-1 text-sm text-ink-soft mb-3">
          <li><strong>DeepSeek</strong> — 一级支持，原生 API。推理流、缓存指标、思考力度控制。</li>
          <li><strong>OpenRouter</strong> — 统一 API，可访问 DeepSeek 等模型。</li>
          <li><strong>OpenAI</strong>、<strong>NVIDIA NIM</strong>、<strong>Volcengine Ark</strong>、<strong>Xiaomi MiMo</strong>、<strong>SiliconFlow</strong>、<strong>Novita</strong>、<strong>Fireworks</strong>、<strong>sglang</strong>、<strong>vLLM</strong>、<strong>Ollama</strong></li>
        </ul>
        <p>
          设置对应的环境变量（如 <code className="inline">OPENROUTER_API_KEY</code>）并在 <code className="inline">~/.codewhale/config.toml</code> 中配置你的提供商。
          自托管 OpenAI 兼容端点可通过 provider 配置接入。
        </p>
      </>
    ),
    sources: ["docs/CONFIGURATION.md", "#1978", "#1710"],
  },
  {
    q: "如何使用 OpenRouter？",
    a: (
      <>
        <pre className="code-block mb-2">
{`# 1. 设置 OpenRouter 密钥
export OPENROUTER_API_KEY=sk-or-v1-...

# 2. 在 ~/.codewhale/config.toml 中：
[providers.openrouter]
api_key = "sk-or-v1-..."

# 3. 使用 OpenRouter 模型运行：
codewhale --model openrouter/deepseek/deepseek-v4-pro

# 或在 config.toml 中设为默认：
default_text_model = "openrouter/deepseek/deepseek-v4-pro"`}
        </pre>
        <p>
          OpenRouter 使用与原生 DeepSeek 提供商相同的推理/缓存解析器。
          模型 ID 遵循 <code className="inline">provider/model-id</code> 格式（如 <code className="inline">openrouter/deepseek/deepseek-v4-flash</code>）。
        </p>
      </>
    ),
    sources: ["docs/CONFIGURATION.md", "#1978"],
  },
  {
    q: "可以使用自托管或本地模型吗（vLLM、Ollama、llama.cpp）？",
    a: (
      <>
        可以。使用 <code className="inline">vllm</code>、<code className="inline">sglang</code> 或 <code className="inline">ollama</code> 提供商连接本地端点。
        对于 OpenAI 兼容端点（llama.cpp server、text-generation-webui 等），可以使用 <code className="inline">openai</code> 提供商并设置自定义 <code className="inline">base_url</code>。
        CodeWhale 也支持 <code className="inline">DEEPSEEK_ALLOW_INSECURE_HTTP=true</code> 用于本地 HTTP 端点。
        完整的 Hugging Face TGI/vLLM 集成正在路线图中。
      </>
    ),
    sources: ["#574", "#1303", "docs/CONFIGURATION.md"],
  },
  {
    q: "Plan、Agent、YOLO 三种模式有什么区别？",
    a: (
      <>
        <ul className="list-disc pl-5 space-y-2 text-sm text-ink-soft">
          <li><strong>Plan（计划）</strong> — 只读调查。可以 grep、读文件、列目录、抓取 URL。不能写入或执行 Shell。</li>
          <li><strong>Agent（代理）</strong> — 默认模式。多步工具调用。Shell 和有副作用的工具根据 approval_mode 设置审批。</li>
          <li><strong>YOLO（全权）</strong> — 自动批准所有操作并启用信任模式。工作区边界解除。请谨慎使用。</li>
        </ul>
        <p className="mt-2">
          按 <kbd className="font-mono text-xs px-1.5 py-0.5 hairline-t hairline-b hairline-l hairline-r">Tab</kbd> 切换模式。
          审批模式（建议 / 自动 / 拒绝）是独立的——例如你可以在 Agent 模式下使用自动审批。
        </p>
      </>
    ),
    sources: ["docs/MODES.md"],
  },
  {
    q: "什么是模型自动路由？Fin 是什么？",
    a: (
      <>
        <p className="mb-2">
          使用 <code className="inline">codewhale --model auto</code> 或 <code className="inline">/model auto</code> 让 CodeWhale 为每个回合自动选择最合适的模型和推理深度。
        </p>
        <p className="mb-2">
          <strong>Fin</strong> 是快速非推理路径（<code className="inline">deepseek-v4-flash</code>，推理关闭），用于路由决策、摘要、RLM 子任务、上下文维护等协调工作。在真实请求发送前，Fin 会做一个小的路由调用来选择具体的模型和推理级别。
        </p>
        <p>
          简短简单的请求可以留在 Flash + 推理关闭的状态。编码、调试、发布工作、架构设计或安全审查则会提升到 Pro 和/或更高的推理级别。Fin 是 CodeWhale 本地逻辑——上游 API 永远不会收到 <code className="inline">model: "auto"</code>。
        </p>
      </>
    ),
    sources: ["README.md", "#1207"],
  },
  {
    q: "什么是 Goal 模式？现在可用吗？",
    a: (
      <>
        Goal 模式是未来的工作流/标签页方向，用于长时间运行的多步目标——不是当前的 <code className="inline">/goal</code> 命令。
        当前的 <code className="inline">/goal</code> 是一个简单的目标设置器。完整的 Goal 模式（自主多回合任务执行，支持检查点/恢复）已规划但尚未实现。
        关注 <a href="https://github.com/Hmbown/CodeWhale/issues/891" className="body-link">#891</a> 的进展。
      </>
    ),
    sources: ["#891"],
  },
  {
    q: "我的代码安全吗？CodeWhale 使用什么沙箱机制？",
    a: (
      <>
        CodeWhale 完全在你的机器上运行。无遥测，不会将你的代码上传到云端处理。
        沙箱后端：<strong>seatbelt</strong>（macOS）、<strong>landlock</strong>（Linux）、受限令牌（Windows）。
        工作区边界默认为 <code className="inline">--workspace</code>。<code className="inline">/trust</code> 可解除边界。
        审批模式可按会话配置。所有凭证/审批/提权事件写入 <code className="inline">~/.codewhale/audit.log</code>。
      </>
    ),
    sources: ["SECURITY.md", "docs/ARCHITECTURE.md"],
  },
  {
    q: "MCP 服务器如何工作？",
    a: (
      <>
        CodeWhale 是双向 MCP 客户端和服务器。在 <code className="inline">~/.codewhale/mcp.json</code> 中定义服务器。
        工具以 <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code> 形式呈现。你也可以通过 <code className="inline">codewhale mcp</code> 将 CodeWhale 暴露为 MCP 服务器。
        查看 <Link href="/zh/docs#mcp" className="body-link">文档页面</Link> 了解配置示例。
      </>
    ),
    sources: ["docs/MCP.md"],
  },
  {
    q: "如何参与贡献？",
    a: (
      <>
        无需签署 CLA。Fork、用约定式提交（<code className="inline">feat:</code>、<code className="inline">fix:</code> 等）创建分支、通过本地检查、提交 PR。
        维护者亲自阅读每一条内容。从标记为 <code className="inline">good first issue</code> 的议题开始。
        查看 <Link href="/zh/contribute" className="body-link">贡献页面</Link> 和 <a href="https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md" className="body-link">CONTRIBUTING.md</a>。
      </>
    ),
    sources: ["CONTRIBUTING.md"],
  },
  {
    q: "我在国内，安装很慢怎么办？",
    a: (
      <>
        使用镜像源：
        <pre className="code-block my-2">
{`# npm 镜像
npm config set registry https://registry.npmmirror.com
npm install -g codewhale

# Cargo 镜像（清华 TUNA）
# 在 ~/.cargo/config.toml 中添加：
[source.crates-io]
replace-with = "tuna"
[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"`}
        </pre>
        <p>
          也可以从 <a href="https://github.com/Hmbown/CodeWhale/releases" className="body-link">GitHub Releases</a> 直接下载预编译二进制。
          Gitee 镜像和 CNB 镜像也可能可用。
        </p>
      </>
    ),
    sources: ["README.md", "#1914", "docs/CNB_MIRROR.md"],
  },
  {
    q: "首次运行时提示 API 密钥被拒绝或认证错误？",
    a: (
      <>
        <p className="mb-2">运行 <code className="inline">codewhale doctor</code>——它会检查 API 密钥、网络、沙箱和 MCP 服务器。完整报告写入 <code className="inline">~/.codewhale/doctor.log</code>。</p>
        <p className="mb-2">常见原因：</p>
        <ul className="list-disc pl-5 space-y-1 text-sm text-ink-soft">
          <li>Shell 启动文件中的 <code className="inline">DEEPSEEK_API_KEY</code> 已过期——打开新 Shell 或使用 <code className="inline">codewhale auth set</code></li>
          <li>密钥来自错误的提供商——确保密钥与你使用的提供商匹配</li>
          <li>网络连接问题——检查 <code className="inline">curl https://api.deepseek.com/v1/models</code></li>
        </ul>
      </>
    ),
    sources: ["#907", "#1545"],
  },
  {
    q: "Model Lab 是什么？Hugging Face 集成什么时候可用？",
    a: (
      <>
        Model Lab 是规划中的开放模型基础设施层：Hugging Face Hub API 用于模型发现、模型卡片、数据集、safetensors 适配器、推理提供商和 Jobs。
        它尚未完全实现。关注 <a href="https://github.com/Hmbown/CodeWhale/issues/1977" className="body-link">#1977</a> 的进展。
        目前，你可以通过 OpenRouter 提供商或自托管端点使用 Hugging Face 模型。
      </>
    ),
    sources: ["#1977", "docs/MODEL_LAB.md"],
  },
  {
    q: "为什么 token 消耗这么大？/ 缓存命中率为什么低？",
    a: (
      <>
        CodeWhale 每次请求都会发送大量上下文（系统提示、项目说明、工具定义）。
        DeepSeek 的前缀缓存被积极使用——系统提示按最稳定的层级排列以最大化缓存命中。
        如果你发现 token 使用量很高，请检查：是否在简单查询中使用了 <code className="inline">deepseek-v4-pro</code>（更适合用 Flash）？
        模型自动路由（Fin）可以帮助为每个回合选择合适的模型。
        缓存命中率取决于提示的稳定性——修改系统提示或切换模型会重置缓存。
      </>
    ),
    sources: ["#1177", "#1818", "#743"],
  },
  {
    q: "如何更新 CodeWhale？",
    a: (
      <>
        <pre className="code-block mb-2">
{`# 发布二进制更新器（适用于 npm/二进制安装）
codewhale update

# npm
npm install -g codewhale@latest

# Cargo
cargo install codewhale-cli --locked --force

# Homebrew
brew update && brew upgrade deepseek-tui`}
        </pre>
        <p>
          如果通过 npm 安装，<code className="inline">codewhale update</code> 会下载最新发布二进制。
          如果镜像延迟，请从 <a href="https://github.com/Hmbown/CodeWhale/releases" className="body-link">GitHub Releases</a> 直接下载。
        </p>
      </>
    ),
    sources: ["README.md", "#1869", "#1914"],
  },
];

export default async function FaqPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const items = isZh ? faqZh : faqEn;

  return (
    <>
      <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="问" />
          <div className="eyebrow">{isZh ? "常见问题" : "FAQ"}</div>
        </div>
        <h1 className="font-display tracking-crisp">
          {isZh ? (
            <>常见问题 <span className="font-cjk text-indigo text-5xl ml-2">FAQ</span></>
          ) : (
            <>FAQ <span className="font-cjk text-indigo text-5xl ml-2">常见问题</span></>
          )}
        </h1>
        <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
          {isZh
            ? "答案来自实际代码、文档、发布说明和 GitHub 议题。每个回答下方标注了信息来源。如有未覆盖的问题，请在 GitHub 上提交 Issue。"
            : "Answers sourced from real code, docs, release notes, and GitHub issues. Sources are cited below each answer. If your question isn't covered, open an issue on GitHub."}
        </p>
      </section>

      <section className="mx-auto max-w-[1400px] px-6 pb-20">
        <div className="space-y-0 hairline-t hairline-b">
          {items.map((item, i) => (
            <details key={i} className="group hairline-b last:border-b-0">
              <summary className="px-0 py-5 cursor-pointer flex items-start gap-4 hover:text-indigo transition-colors">
                <span className="font-mono text-indigo tabular text-sm pt-0.5 shrink-0">
                  {String(i + 1).padStart(2, "0")}
                </span>
                <span className="font-display text-lg leading-snug flex-1">{item.q}</span>
                <span className="font-mono text-ink-mute text-sm group-open:rotate-45 transition-transform shrink-0">+</span>
              </summary>
              <div className="pb-5 pl-10 pr-4">
                <div className={`text-ink-soft leading-relaxed ${isZh ? "leading-[1.9] tracking-wide" : ""}`}>
                  {item.a}
                </div>
                {item.sources && item.sources.length > 0 && (
                  <div className="mt-3 flex items-center gap-2 flex-wrap">
                    <span className="font-mono text-[0.66rem] text-ink-mute uppercase tracking-wider">
                      {isZh ? "来源" : "Sources"}:
                    </span>
                    {item.sources.map((s) => (
                      <span key={s} className="font-mono text-[0.7rem] text-indigo">{s}</span>
                    ))}
                  </div>
                )}
              </div>
            </details>
          ))}
        </div>

        <div className="mt-12 text-center">
          <p className="text-ink-soft text-sm mb-4">
            {isZh
              ? "没找到你的问题？"
              : "Didn't find your question?"}
          </p>
          <a
            href="https://github.com/Hmbown/CodeWhale/issues/new/choose"
            className="inline-flex items-center gap-2 px-5 py-3 bg-ink text-paper font-mono text-sm uppercase tracking-wider hover:bg-indigo transition-colors"
          >
            {isZh ? "提交 Issue →" : "Open an issue →"}
          </a>
        </div>
      </section>
    </>
  );
}
