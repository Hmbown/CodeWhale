import { RefRows, withCodeSpans } from "@/components/code-spans";
import { getChrome, getDocsTrust } from "@/lib/i18n/dictionaries";
import { buildPageMetadata, SITE_SECURITY_EMAIL } from "@/lib/page-meta";

/** Code-owned literals from docs/SANDBOX.md and docs/TELEMETRY.md. Not copy. */
const SPANS: Record<string, string> = {
  seatbelt: "macos-seatbelt",
  preferBwrap: "prefer_bwrap = true",
  bwrap: "linux-bwrap",
  none: "none",
  opensandbox: 'sandbox_backend = "opensandbox"',
  endpoint: "https://telemetry.codewhale.net/v1/telemetry",
  dryRun: 'telemetry_endpoint = ""',
  dryRunFile: "$CODEWHALE_HOME/telemetry/dryrun.jsonl",
  configOff: "codewhale config set telemetry false",
  envOff: "CODEWHALE_TELEMETRY=0",
  auditLog: "$CODEWHALE_HOME/audit.log",
};

/** The one security contact the spine publishes (lib/page-meta.ts). */
const SECURITY_MAILTO = `mailto:${SITE_SECURITY_EMAIL}`;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsTrust(locale);
  return buildPageMetadata({
    path: "/docs/trust",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function TrustPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsTrust(locale);
  const chrome = getChrome(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
      </section>

      <section id="boundary" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-4">{t.boundaryTitle}</h2>
        <RefRows rows={t.boundaries} />
      </section>

      <section id="approvals" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.approvalTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.approvalLead}</p>
      </section>

      <section id="sandbox" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.sandboxTitle}</h2>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{t.sandboxLead}</p>
        <RefRows rows={t.sandboxes} spans={SPANS} />
        <p className={`${t.bodyClassName} mt-4 text-sm`}>{t.sandboxNote}</p>
      </section>

      <section id="telemetry" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.telemetryTitle}</h2>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{t.telemetryLead}</p>
        <RefRows rows={t.telemetry} spans={SPANS} />
      </section>

      <section id="audit" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.auditTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.auditLead, SPANS)}</p>
      </section>

      <section id="report" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.reportTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.reportLead}</p>
        <div className="portal-actions">
          <a className="portal-button portal-button-primary" href={SECURITY_MAILTO}>
            {t.reportCta} · {chrome.footerSecurity}
          </a>
        </div>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
