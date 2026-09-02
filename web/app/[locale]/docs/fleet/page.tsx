import { Fragment } from "react";
import { PRODUCT_TERMS } from "@/lib/content/vocabulary";
import { getDocsFleet, pickText, splitTokens } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

const CODE_SPANS: Record<string, string> = {
  fleetSaved: "/fleet saved",
  fleetStatusTui: "/fleet status",
  fleetWorkers: "/fleet workers",
  subagents: "/subagents",
  fleetStatusShell: "codewhale fleet status",
};

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsFleet(locale);
  return buildPageMetadata({
    path: "/docs/fleet",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function FleetPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsFleet(locale);
  const vocabulary = PRODUCT_TERMS.map((row) => ({
    term: row.term,
    definition: pickText(row.long, locale),
  }));

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
        <div className="hairline-t mt-6">
          {vocabulary.map((row) => (
            <section key={row.term} className="py-4 hairline-b">
          <h2 className="font-display text-xl">{row.term}</h2>
              <p className={`${t.bodyClassName} mt-1 text-sm`}>{row.definition}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="cli" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.runTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.runLead}</p>
        <pre className="code-block mt-4">{`codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all`}</pre>
        <p className={`${t.bodyClassName} mt-3`}>
          {splitTokens(t.statusLead).map((part, i) =>
            "token" in part ? (
              <code key={`${i}-${part.token}`} className="inline">
                {CODE_SPANS[part.token] ?? `{${part.token}}`}
              </code>
            ) : (
              <Fragment key={`${i}-text`}>{part.text}</Fragment>
            ),
          )}
        </p>
      </section>

      <section id="profiles" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.profilesTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>
          {splitTokens(t.profilesLead).map((part, i) =>
            "token" in part ? (
              <code key={`${i}-${part.token}`} className="inline">
                {CODE_SPANS[part.token] ?? `{${part.token}}`}
              </code>
            ) : (
              <Fragment key={`${i}-text`}>{part.text}</Fragment>
            ),
          )}
        </p>
      </section>

      <section id="workflow" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.workflowTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.workflowLead}</p>
        <p className={`${t.bodyClassName} mt-3`}>{t.workflowLimits}</p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
