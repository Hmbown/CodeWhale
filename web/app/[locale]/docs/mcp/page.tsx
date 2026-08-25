import { Fragment } from "react";
import { getDocsMcp, splitTokens } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

const CODE_SPANS: Record<string, string> = {
  configPath: "~/.codewhale/mcp.json",
  legacyConfigPath: "~/.deepseek/mcp.json",
  configPathOption: "mcp_config_path",
  configEnvVar: "DEEPSEEK_MCP_CONFIG",
  serversKey: "mcpServers",
  initCommand: "codewhale mcp init",
  mcpCommand: "/mcp",
  toolNamePattern: "mcp_<server>_<tool>",
  gitServer: "git",
  statusTool: "status",
  gitStatusTool: "mcp_git_status",
  serveMcp: "codewhale serve --mcp",
  mcpServerCommand: "codewhale mcp-server",
  addSelfCommand: "codewhale mcp add-self",
  serveHttp: "serve --http",
};

function withCodeSpans(template: string) {
  return splitTokens(template).map((part, i) =>
    "token" in part ? (
      <code key={`${i}-${part.token}`} className="inline">
        {CODE_SPANS[part.token] ?? `{${part.token}}`}
      </code>
    ) : (
      <Fragment key={`${i}-text`}>{part.text}</Fragment>
    ),
  );
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsMcp(locale);
  return buildPageMetadata({
    path: "/docs/mcp",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function McpPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsMcp(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">MCP</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.overviewConfig)}</p>
      </section>

      <section id="setup" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.setupTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.setupLead)}</p>
        <pre className="code-block mt-4">{`codewhale mcp add <name> --command "<cmd>" --arg "<arg>"
codewhale mcp add <name> --url "https://example.com/mcp" --bearer-token-env-var MCP_TOKEN
codewhale mcp login <name>      # OAuth for remote servers
codewhale mcp list
codewhale mcp validate`}</pre>
        <p className={`${t.bodyClassName} mt-3`}>{t.setupReload}</p>
      </section>

      <section id="auth" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.authTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.authLead}</p>
      </section>

      <section id="tools" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.toolsTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.toolsLead)}</p>
        <p className={`${t.bodyClassName} mt-3`}>{t.toolsTrust}</p>
      </section>

      <section id="server" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.serverTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.serverLead)}</p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
