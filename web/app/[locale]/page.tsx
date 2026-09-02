import { Fragment } from "react";
import Image from "next/image";
import Link from "next/link";
import { GettingStartedSteps } from "@/components/getting-started-steps";
import { InstallCodeBlock } from "@/components/install-code-block";
import { RevealOnScroll } from "@/components/reveal-on-scroll";
import { Whale } from "@/components/whale";
import { getFacts } from "@/lib/facts";
import { fill, getHome, splitToken } from "@/lib/i18n/dictionaries";
import { REPO_ISSUES_URL, REPO_RELEASES_URL, REPO_URL, DISCORD_URL } from "@/lib/i18n/links";
import { serializeJsonLd } from "@/lib/json-ld";
import { buildSoftwareApplicationJsonLd } from "@/lib/software-application-schema";

// Revalidate against source-proven runtime facts without giving up static edge
// caching. `getFacts()` rejects legacy or older KV snapshots.
export const revalidate = 300;

/**
 * The newspaper-ocean homepage.
 *
 * Every visible string resolves through `getHome(locale)`
 * — English, Chinese, and every other routed locale take the identical path,
 * with the English dictionary as the build-time-guaranteed fallback. The only
 * literals left in this file are code-owned per docs/VOICE.md: the product
 * control vocabulary (`Plan · Work · Operate`, `Ask · Auto-Review · Full
 * Access`, `TUI · exec · web · API`), the install command, `cargo test
 * --locked`, package-manager and mirror proper nouns, and the screenshot path.
 */
export default async function HomePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const d = getHome(locale);
  const facts = await getFacts();
  const sourceVersion = facts.version ?? "unknown";
  const publishedRelease = facts.latestPublishedRelease;
  const sourceIsPublished = publishedRelease?.version === sourceVersion;
  const providerCount = facts.providers.length;
  const providerRoutes = fill(d.providerRoutes, { count: providerCount });

  // The install URL resolves published artifacts, so its structured version
  // must come from the published-release receipt rather than source-candidate
  // facts. When no release is known, the schema omits softwareVersion.
  const jsonLd = buildSoftwareApplicationJsonLd(publishedRelease);

  // The lede typesets the brand in its own span. Splitting on the {brand}
  // token keeps the sentence a single translated unit — no concatenation of
  // fragments around a variable, and a locale may place the brand anywhere.
  const ledeParts = splitToken(d.heroIntro, "brand");

  return (
    <div className="product-home paper-home">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: serializeJsonLd(jsonLd) }}
      />
      {/* Entrance motion for the below-fold sections: the marks are the
          data-reveal / data-reveal-group attributes in this tree; the
          observer does the rest, and reduced motion skips it wholesale. */}
      <RevealOnScroll />
      {/* HERO — newspaper split: claim + live terminal proof */}
      <section className="hero">
        <div className="product-container product-hero-grid paper-hero-grid">
          <div className="product-hero-copy paper-hero-copy">
            <div className="mb-5">
              <span className="eyebrow">{d.kicker}</span>
            </div>

            <h1 className="font-display tracking-crisp">
              {d.heroTitleA}
              <br />
              <span className="paper-hero-accent">{d.heroTitleB}</span>
            </h1>

            <p className="paper-hero-lede">
              {ledeParts.map((part, index) => (
                <Fragment key={index}>
                  {index > 0 && (
                    <span className="font-cjk text-indigo font-semibold">Codewhale</span>
                  )}
                  {part}
                </Fragment>
              ))}
            </p>

            <div className="product-actions paper-actions">
              <Link href={`/${locale}/install`} className="product-button product-button-primary">
                {d.install} <span aria-hidden>→</span>
              </Link>
              <Link href={`/${locale}/docs`} className="product-button">
                {d.docs} <span aria-hidden>→</span>
              </Link>
              <a href={REPO_URL} className="product-button product-button-ghost">
                GitHub
              </a>
            </div>

            <div className="product-install paper-install">
              <div className="eyebrow mb-2">{d.installEyebrow}</div>
              <InstallCodeBlock
                cmd="npm install -g codewhale"
                copyLabel={d.copy}
                copiedLabel={d.copied}
              />
              <div className="paper-install-meta">
                <span>{d.installRequirement}</span>
                <Link href={`/${locale}/install`} className="text-indigo hover:underline">
                  {d.installOtherWays}
                </Link>
              </div>
            </div>

            {/*
              The TUI header grammar: a `cw` chip and a dot chain. Each fact is
              its own translated unit — the separators are CSS punctuation, so
              nothing is concatenated around a token and no locale inherits an
              English joining word. `cw` is the binary's own name, code-owned
              exactly like `Codewhale`.
            */}
            <p
              className="product-facts paper-facts dotline"
              data-source-state={sourceIsPublished ? "published release" : "source candidate"}
              data-source-state-label={sourceIsPublished ? d.publishedRelease : d.figcaptionSourceCandidate}
            >
              <span className="dotline-chip">cw</span>
              <span>
                {publishedRelease
                  ? fill(d.latestRelease, { tag: publishedRelease.tag })
                  : d.releaseUnavailable}
              </span>
              <span>
                {`${sourceIsPublished ? d.currentSource : d.sourceCandidate} v${sourceVersion}`}
              </span>
              <span>{providerRoutes}</span>
              <span>{facts.license ?? "MIT"}</span>
            </p>
          </div>

          <figure className="product-shot paper-shot">
            <div className="product-shot-toolbar paper-shot-toolbar">
              <span>
                <Whale size={18} />
                Codewhale TUI
              </span>
              <span>{d.shotSession}</span>
            </div>
            <Image
              src="/codewhale-tui.webp"
              alt={d.screenshotAlt}
              width={1562}
              height={1256}
              sizes="(max-width: 900px) calc(100vw - 2rem), 52vw"
              priority
            />
            <figcaption>{d.figcaption}</figcaption>
          </figure>
        </div>
      </section>

      {/* Proof strip */}
      <section className="product-proof paper-proof">
        <div className="product-container product-proof-grid" data-reveal>
          <h2 className="font-display">{d.proofHeading}</h2>
          <p>{d.proofBody}</p>
        </div>
      </section>

      {/* Workflow */}
      <section className="product-workflow paper-workflow">
        <div className="product-container">
          <div className="flex items-baseline gap-4 mb-6 hairline-b pb-4" data-reveal>
            <h2 className="font-display">{d.workflowHeading}</h2>
          </div>
          <ol className="product-workflow-steps" data-reveal-group>
            {d.workflow.map(([title, description], index) => (
              <li key={title}>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <h3>{title}</h3>
                <p>{description}</p>
              </li>
            ))}
          </ol>
        </div>
      </section>

      {/* Getting started */}
      <section className="product-start paper-start">
        <div className="product-container">
          <div className="flex items-baseline gap-4 mb-4 hairline-b pb-4" data-reveal>
            <h2 className="font-display">{d.startHeading}</h2>
          </div>
          <p className="product-start-lede" data-reveal>{d.startLede}</p>
          <GettingStartedSteps locale={locale} />
          <div className="product-start-links" data-reveal>
            <Link href={`/${locale}/docs/guide`}>{d.startGuideLink}</Link>
            <Link href={`/${locale}/docs/vocabulary`}>{d.startVocabularyLink}</Link>
          </div>
        </div>
      </section>

      {/* Boundaries */}
      <section className="product-boundaries paper-boundaries">
        <div className="product-container product-boundaries-grid">
          <div data-reveal>
            <div className="flex items-baseline gap-4 mb-4">
              <h2 className="font-display">
                {d.boundariesHeadingA}
                <br />
                <span>{d.boundariesHeadingB}</span>
              </h2>
            </div>
            <p>{d.boundariesBody}</p>
          </div>
          <dl className="product-boundary-list" data-reveal-group>
            <div>
              <dt>{providerRoutes}</dt>
              <dd>{d.hostedGatewayLocal}</dd>
            </div>
            <div>
              <dt>Plan · Work · Operate</dt>
              <dd>{d.planActOperateDesc}</dd>
            </div>
            <div>
              <dt>Ask · Auto-Review · Full Access</dt>
              <dd>{d.askAutoReviewDesc}</dd>
            </div>
            <div>
              <dt>TUI · exec · web · API</dt>
              <dd>{d.tuiExecWebDesc}</dd>
            </div>
          </dl>
        </div>
      </section>

      {/*
        THE WATERLINE. Everything below here is one water column: a single
        gradient on the wrapper, sampled by absolute page position across the
        three bands inside it, continuing into the footer's identical deep
        stop as the seabed. The bands themselves carry no field of their own.
      */}
      <div className="ocean-column">
        {/* Surfaces */}
        <section className="product-surfaces paper-surfaces">
          <div className="product-container">
            <div className="flex items-baseline gap-4 mb-6 hairline-b pb-4" data-reveal>
              <h2 className="font-display">{d.surfacesHeading}</h2>
            </div>
            <div className="product-surface-list" data-reveal-group>
              {d.surfaces.map(([name, description]) => (
                <div key={name}>
                  <strong>{name}</strong>
                  <span>{description}</span>
                </div>
              ))}
            </div>
            <Link href={`/${locale}/runtime`} data-reveal>{d.runtimeLink}</Link>
          </div>
        </section>

        {/* Install band */}
        <section className="product-install-band paper-install-band">
          <div className="product-container product-install-grid" data-reveal-group>
            <h2 className="font-display">{d.installBandHeading}</h2>
            <div>
              {/* The composer plate. `❯` is a code-owned literal, like the
                  install command it prompts for — it is the product's glyph,
                  not a sentence, and no locale renders it differently. */}
              <div className="product-composer">
                <span className="product-composer-prompt" aria-hidden>
                  ❯
                </span>
                <InstallCodeBlock
                  cmd="npm install -g codewhale"
                  copyLabel={d.copy}
                  copiedLabel={d.copied}
                />
              </div>
              {/* Already a dot chain in every locale; the separators just stop
                  being characters in the markup and become CSS punctuation. */}
              <p className="dotline">
                <span>Cargo</span>
                <span>{d.binaries}</span>
                <span>Docker</span>
                <span>Nix</span>
                <span>Windows</span>
                <span>Android / Termux</span>
                <span>{d.chinaMirrors}</span>
              </p>
              <Link href={`/${locale}/install`}>{d.installGuideLink}</Link>
            </div>
          </div>
        </section>

        {/* Community */}
        <section className="product-community paper-community">
          <div className="product-container product-community-grid" data-reveal>
            <div>
              <h2 className="font-display">{d.communityHeading}</h2>
              <p>{d.communityBody}</p>
            </div>
            <nav aria-label={d.communityLinksAria}>
              <a href={REPO_URL}>GitHub</a>
              <a href={REPO_ISSUES_URL}>Issues</a>
              <a href={DISCORD_URL}>Discord</a>
              <Link href={`/${locale}/contribute`}>{d.contribute}</Link>
              {publishedRelease ? (
                <a href={publishedRelease.url}>{publishedRelease.tag}</a>
              ) : (
                <a href={REPO_RELEASES_URL}>Releases</a>
              )}
            </nav>
          </div>
        </section>
      </div>
    </div>
  );
}
