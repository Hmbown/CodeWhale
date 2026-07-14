import Link from "next/link";
import { InstallCodeBlock } from "@/components/install-code-block";
import { Whale } from "@/components/whale";
import { DOC_TOPICS, REPO_DOCS_BASE, type DocTopic } from "@/lib/docs-map";
import { getFacts } from "@/lib/facts";

const START_TOPIC_IDS = ["install", "guide", "configuration", "modes"];
const RUNTIME_TOPIC_IDS = ["tools", "sandbox", "providers", "subagents"];
const EXTEND_TOPIC_IDS = ["runtime-api", "mcp", "fleet", "troubleshooting"];

function topics(ids: string[]): DocTopic[] {
  return ids.flatMap((id) => {
    const topic = DOC_TOPICS.find((candidate) => candidate.id === id);
    return topic ? [topic] : [];
  });
}

function topicHref(topic: DocTopic, locale: string): string {
  if (topic.id === "install") return `/${locale}/install`;
  if (topic.id === "providers") return `/${locale}/models`;
  if (topic.hasPage) return `/${locale}/docs/${topic.slug}`;

  const source = Array.isArray(topic.repoSource) ? topic.repoSource[0] : topic.repoSource;
  return `${REPO_DOCS_BASE}/${source}`;
}

function TopicList({ items, locale }: { items: DocTopic[]; locale: string }) {
  const isZh = locale === "zh";

  return (
    <div className="portal-topic-list">
      {items.map((topic) => {
        const external = !topic.hasPage && topic.id !== "install" && topic.id !== "providers";
        return (
          <Link
            key={topic.id}
            href={topicHref(topic, locale)}
            target={external ? "_blank" : undefined}
            rel={external ? "noreferrer" : undefined}
          >
            <strong>{isZh ? topic.label.zh : topic.label.en}</strong>
            <span>{isZh ? topic.description.zh : topic.description.en}</span>
            <span aria-hidden="true">{external ? "↗" : "→"}</span>
          </Link>
        );
      })}
    </div>
  );
}

export default async function HomePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const facts = await getFacts();

  return (
    <div className="portal-home">
      <section className="portal-hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container portal-hero-grid">
          <div className="portal-hero-copy">
            <div className="portal-mark">
              <Whale size={28} className="text-current" />
              <span>{isZh ? "Codewhale 文档" : "Codewhale documentation"}</span>
            </div>
            <h1>{isZh ? "Codewhale 文档" : "Codewhale documentation"}</h1>
            <p className="portal-lede">
              {isZh
                ? "安装这个开源终端编程智能体，连接你已有的模型提供商，并在需要时查找有关模式、权限、工具、配置和运行时集成的准确说明。"
                : "Install the open-source terminal coding agent, connect the provider you already use, and find precise guidance for modes, permissions, tools, configuration, and runtime integrations."}
            </p>
            <div className="portal-actions">
              <Link href={`/${locale}/docs`} className="portal-button portal-button-primary">
                {isZh ? "浏览文档" : "Browse the documentation"}
              </Link>
              <Link href={`/${locale}/install`} className="portal-button portal-button-secondary">
                {isZh ? "查看安装指南" : "Read the installation guide"}
              </Link>
            </div>
            <p className="portal-meta">
              {isZh
                ? `当前运行时版本为 ${facts.version ?? "0.8.x"}，支持 ${facts.providers.length} 个提供商，并采用 ${facts.license ?? "MIT"} 许可证。`
                : `The current runtime is version ${facts.version ?? "0.8.x"}, supports ${facts.providers.length} providers, and is licensed under ${facts.license ?? "MIT"}.`}
            </p>
          </div>

          <aside className="portal-quickstart" aria-labelledby="quickstart-heading">
            <span>{isZh ? "快速开始" : "Quickstart"}</span>
            <h2 id="quickstart-heading">
              {isZh ? "安装 CLI 与交互式 TUI。" : "Install the CLI and interactive TUI."}
            </h2>
            <p>
              {isZh
                ? "npm 软件包会安装两个可执行文件；完整安装指南还包括 Cargo、Homebrew、Docker 和直接下载。"
                : "The npm package installs both executables. The full guide also covers Cargo, Homebrew, Docker, and direct downloads."}
            </p>
            <InstallCodeBlock
              cmd="npm install -g codewhale"
              copyLabel={isZh ? "复制" : "Copy"}
              copiedLabel={isZh ? "已复制 ✓" : "Copied ✓"}
            />
            <Link href={`/${locale}/install`}>
              {isZh ? "阅读安装与首次运行说明 →" : "Read installation and first-run guidance →"}
            </Link>
          </aside>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>{isZh ? "从这里开始" : "Start here"}</span>
            <h2>{isZh ? "开始使用运行时。" : "Get started with the runtime."}</h2>
            <p>
              {isZh
                ? "先选择安装方式和提供商，然后阅读模式与配置说明，了解 Codewhale 在修改代码之前会如何工作。"
                : "Choose an installation path and provider first, then read the mode and configuration guidance so you know how Codewhale will behave before it changes code."}
            </p>
          </div>
          <TopicList items={topics(START_TOPIC_IDS)} locale={locale} />
        </div>
      </section>

      <section className="portal-section portal-section-muted">
        <div className="portal-container">
          <div className="portal-docs-heading">
            <div>
              <span>{isZh ? "文档索引" : "Documentation index"}</span>
              <h2>{isZh ? "浏览 Codewhale 文档。" : "Browse the Codewhale documentation."}</h2>
            </div>
            <Link href={`/${locale}/docs`}>{isZh ? "查看全部文档 →" : "View all documentation →"}</Link>
          </div>

          <div className="portal-doc-groups">
            <section>
              <h3>{isZh ? "使用运行时" : "Use the runtime"}</h3>
              <p>
                {isZh
                  ? "了解工具、审批边界、提供商和子 Agent 在一次实际会话中如何协同。"
                  : "Understand how tools, approval boundaries, providers, and sub-agents work together in a real session."}
              </p>
              <TopicList items={topics(RUNTIME_TOPIC_IDS)} locale={locale} />
            </section>
            <section>
              <h3>{isZh ? "扩展与运维" : "Extend and operate"}</h3>
              <p>
                {isZh
                  ? "使用运行时 API、MCP、Fleet 和故障排除资料把 Codewhale 接入更大的工作流。"
                  : "Use the runtime API, MCP, Fleet, and troubleshooting material when Codewhale becomes part of a larger workflow."}
              </p>
              <TopicList items={topics(EXTEND_TOPIC_IDS)} locale={locale} />
            </section>
          </div>
        </div>
      </section>

      <section className="portal-community">
        <div className="portal-container portal-community-grid">
          <div>
            <span>{isZh ? "国际开源社区" : "An international open-source community"}</span>
            <h2>{isZh ? "Codewhale 由国际社区共同构建。" : "Codewhale is built by an international community."}</h2>
          </div>
          <div>
            <p>
              {isZh
                ? "Codewhale 由不同时区、语言和技术背景的贡献者公开构建。如果某个行为不清楚，请提交带有复现步骤的 issue；如果你能改进运行时、文档或测试，请发送 pull request。"
                : "Codewhale is built in public by contributors working across time zones, languages, and technical backgrounds. If behavior is unclear, file an issue with a reproduction; if you can improve the runtime, documentation, or tests, send a pull request."}
            </p>
            <div className="portal-community-links">
              <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose">
                {isZh ? "提交 issue →" : "File an issue →"}
              </Link>
              <Link href={`/${locale}/contribute`}>
                {isZh ? "阅读贡献指南 →" : "Read the contribution guide →"}
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/pulls">
                {isZh ? "查看 pull requests →" : "Browse pull requests →"}
              </Link>
              <Link href={`/${locale}/community`}>
                {isZh ? "了解社区 →" : "Meet the community →"}
              </Link>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
