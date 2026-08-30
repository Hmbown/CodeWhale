import { RefRows, withCodeSpans } from "@/components/code-spans";
import { getDocsComputers } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

/** Code-owned literals from docs/DAYTONA_CLOUD_DISPATCH.md. Not copy. */
const SPANS: Record<string, string> = {
  cloudAgent: "codewhale cloud-agent",
  dispatch: "codewhale dispatch",
  kind: "kind=cloud",
  remoteFlag: "--remote",
  apiKey: "export DAYTONA_API_KEY=…",
  apiUrl: "DAYTONA_API_URL",
  slot: "daytona",
  alias: "CWC_DAYTONA_TOKEN",
  status: "codewhale dispatch --status",
  bare: "/dispatch",
  proposed: "proposed",
  refused: "refused",
  login: "codewhale login",
};

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsComputers(locale);
  return buildPageMetadata({
    path: "/docs/computers",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function ComputersPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsComputers(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
      </section>

      <section id="propose" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.proposeTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.proposeLead, SPANS)}</p>
        <pre className="code-block mt-4">{`codewhale dispatch "open a PR that fixes the flake" --remote github
codewhale dispatch --confirm cloud_<id>

# the same two steps inside the TUI
/dispatch open a PR that fixes the flake --remote github
/dispatch confirm cloud_<id>`}</pre>
        <p className={`${t.bodyClassName} mt-4`}>{withCodeSpans(t.jobsLead, SPANS)}</p>
        <pre className="code-block mt-4">{`/jobs list
/dispatch list
/dispatch show <id>
/dispatch cancel <id>
codewhale dispatch --list`}</pre>
      </section>

      <section id="remotes" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.remotesTitle}</h2>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{withCodeSpans(t.remotesLead, SPANS)}</p>
        <RefRows rows={t.remotes} spans={SPANS} />
      </section>

      <section id="enable" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.enableTitle}</h2>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{t.enableLead}</p>
        <RefRows rows={t.enableSteps} spans={SPANS} />
        <p className={`${t.bodyClassName} mt-4 text-sm`}>{withCodeSpans(t.cliNote, SPANS)}</p>
      </section>

      <section id="rules" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-4">{t.rulesTitle}</h2>
        <RefRows rows={t.rules} spans={SPANS} />
      </section>

      <section id="membership" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.membershipTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.membershipLead, SPANS)}</p>
      </section>

      <section id="leftover" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-4">{t.leftoverTitle}</h2>
        <RefRows rows={t.leftover} spans={SPANS} />
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
