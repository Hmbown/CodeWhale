import { RefRows, withCodeSpans } from "@/components/code-spans";
import { getDocsAuth } from "@/lib/i18n/dictionaries";
import { APP_LOGIN_URL, APP_SIGNUP_URL } from "@/lib/i18n/links";
import { buildPageMetadata } from "@/lib/page-meta";

/** Code-owned literals from docs/CONFIGURATION.md. Not copy. */
const SPANS: Record<string, string> = {
  authSet: "codewhale auth set --provider <provider>",
  apiKeyFlag: "--api-key",
  login: "codewhale login",
  accountLogin: "codewhale account login",
  profile: "--profile",
  cloud: "codewhale cloud …",
  fileStoreEnv: "CODEWHALE_CLOUD_ALLOW_FILE_SESSION_STORE",
  keys: "codewhale account keys list|set|remove",
  portable: "codewhale config export --portable",
};

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsAuth(locale);
  return buildPageMetadata({
    path: "/docs/auth",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function AuthPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsAuth(locale);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{t.overviewLead}</p>
        <RefRows rows={t.credentials} spans={SPANS} />
      </section>

      <section id="provider-keys" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.providerTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.providerLead, SPANS)}</p>
        <pre className="code-block mt-4">{`codewhale auth set --provider deepseek
codewhale --model deepseek-v4-flash`}</pre>
      </section>

      <section id="account" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.accountTitle}</h2>
        <p className={`${t.bodyClassName} mt-3 mb-4`}>{withCodeSpans(t.accountLead, SPANS)}</p>
        <RefRows rows={t.accountCommands} termSpans />
      </section>

      <section id="storage" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.storageTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.storageLead, SPANS)}</p>
      </section>

      <section id="vault" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.vaultTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.vaultLead, SPANS)}</p>
      </section>

      <section id="portable" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.portableTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.portableLead, SPANS)}</p>
      </section>

      <section id="web" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.appTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.appLead}</p>
        <div className="portal-actions">
          <a className="portal-button portal-button-primary" href={APP_LOGIN_URL}>
            {t.appSignIn}
          </a>
          <a className="portal-button portal-button-secondary" href={APP_SIGNUP_URL}>
            {t.appRegister}
          </a>
        </div>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
