import { Fragment } from "react";
import { getDocsWeb, splitTokens } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

/**
 * The commands, flags and addresses the overview and local paragraphs typeset
 * as inline `<code>`. `docs/VOICE.md` keeps commands, flags and paths
 * code-owned, so the dictionaries carry `{token}`s rather than the literals.
 */
const CODE_SPANS: Record<string, string> = {
  webCommand: "codewhale web",
  loopbackHost: "127.0.0.1",
  defaultUrl: "http://127.0.0.1:7878",
  portExample: "codewhale web --port 8788",
  portFlag: "--port",
  tailscaleFlag: "--tailscale",
  tailscaleCommand: "codewhale web --tailscale",
  hostFlag: "--host",
  mobileCommand: "codewhale app-server --mobile",
  httpFlag: "--http",
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
  const t = getDocsWeb(locale);
  return buildPageMetadata({
    path: "/docs/web",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function WebClientPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsWeb(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.overviewLead)}</p>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewBody}</p>
      </section>

      <section id="auth" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.authTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.authLead}</p>
      </section>

      <section id="local" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.localTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.localLead)}</p>
      </section>

      <section id="tailscale" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.tailscaleTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.tailscaleLead)}</p>
      </section>

      <section id="troubleshooting" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.troubleshootingTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.troubleshootingLead}</p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
