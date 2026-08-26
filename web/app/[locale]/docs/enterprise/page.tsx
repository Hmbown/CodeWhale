import { getDocsEnterprise } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsEnterprise(locale);
  return buildPageMetadata({
    path: "/docs/enterprise",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function EnterprisePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsEnterprise(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
        <div className="hairline-t mt-6">
          {t.facts.map(([name, detail]) => (
            <section key={name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{name}</h3>
              <p className={`${t.bodyClassName} mt-1 text-sm`}>{detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="telemetry" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.telemetryTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.telemetryLead}</p>
      </section>

      <section id="controls" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.controlsTitle}</h2>
        <div className="hairline-t mt-6">
          {t.controls.map(([name, detail]) => (
            <section key={name} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{name}</h3>
              <p className={`${t.bodyClassName} mt-1 text-sm`}>{detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="credentials" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.credentialsTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.credentialsLead}</p>
      </section>

      <section id="policy" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.policyTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.policyLead}</p>
      </section>

      <section id="airgap" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.airgapTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.airgapLead}</p>
        <pre className="code-block mt-4">{`telemetry = false

[update]
check_for_updates = false`}</pre>
      </section>

      <section id="sessions" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.sessionsTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.sessionsLead}</p>
        <pre className="code-block mt-4">
          {"CODEWHALE_RUNTIME_DIR=$CODEWHALE_HOME/tasks/runtime-$SESSION_ID codewhale"}
        </pre>
      </section>

      <section id="claims" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.claimsTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.claimsLead}</p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
