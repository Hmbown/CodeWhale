import { Fragment } from "react";
import Image from "next/image";
import Link from "next/link";
import { GettingStartedSteps } from "@/components/getting-started-steps";
import { InstallCodeBlock } from "@/components/install-code-block";
import { Strata } from "@/components/strata";
import { getFacts } from "@/lib/facts";
import { GETTING_STARTED_STEPS } from "@/lib/content/getting-started";
import { fill, getHome, splitToken } from "@/lib/i18n/dictionaries";
import {
  APP_SIGNUP_URL,
  DISCORD_URL,
  REPO_ISSUES_URL,
  REPO_RELEASES_URL,
  REPO_URL,
} from "@/lib/i18n/links";
import { serializeJsonLd } from "@/lib/json-ld";
import { buildSoftwareApplicationJsonLd } from "@/lib/software-application-schema";

// Revalidate against source-proven runtime facts without giving up static edge
// caching. `getFacts()` rejects legacy or older KV snapshots.
export const revalidate = 300;

/**
 * The Tidal Folio homepage: a sheet read under the sea.
 *
 * Every visible string resolves through `getHome(locale)` — English,
 * Chinese, and every other routed locale take the identical path, with the
 * English dictionary as the build-time-guaranteed fallback. The only literals
 * left in this file are code-owned per docs/VOICE.md: the product control
 * vocabulary (`Plan · Work · Operate`, `Ask · Auto-Review · Full Access`),
 * the install command, package-manager and mirror proper nouns, the chapter
 * numerals, and the screenshot path.
 */
export default async function HomePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const d = getHome(locale);
  const facts = await getFacts();
  const sourceVersion = facts.version ?? "unknown";
  const publishedRelease = facts.latestPublishedRelease;
  const sourceIsPublished = publishedRelease?.version === sourceVersion;

  // The install URL resolves published artifacts, so its structured version
  // must come from the published-release receipt rather than source-candidate
  // facts. When no release is known, the schema omits softwareVersion.
  const jsonLd = buildSoftwareApplicationJsonLd(publishedRelease);

  // The lede typesets the brand in its own span. Splitting on the {brand}
  // token keeps the sentence a single translated unit — no concatenation of
  // fragments around a variable, and a locale may place the brand anywhere.
  const ledeParts = splitToken(d.heroIntro, "brand");

  return (
    <div className="product-home">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: serializeJsonLd(jsonLd) }}
      />

      {/* THE PLATE — paper at the top left, the water rising from the bottom
          right, the real terminal floating at the waterline. */}
      <section className="folio-hero">
        <Strata variant="hero" />
        <div className="product-container folio-hero-grid">
          <div className="folio-hero-copy">
            <p className="folio-kicker">{d.kicker}</p>
            <h1>
              {d.heroTitleA}
              <br />
              {d.heroTitleB}
            </h1>
            <p className="folio-lede">
              {ledeParts.map((part, index) => (
                <Fragment key={index}>
                  {index > 0 && <span className="font-semibold text-ink">Codewhale</span>}
                  {part}
                </Fragment>
              ))}
            </p>
            <div className="folio-actions">
              <Link href={`/${locale}/install`} className="folio-button folio-button-primary">
                {d.getCodewhale}
              </Link>
              <Link href={`/${locale}/product`} className="folio-button">
                {d.exploreProduct}
              </Link>
            </div>
          </div>

          {/* The founder's own capture of the v0.9.12 development build: a
              new session, Work mode, Full Access. Shown at its native aspect
              so the footer stays legible; the caption says exactly what it
              is and is not. */}
          <figure className="folio-shot">
            <Image
              src="/codewhale-tui.png"
              alt={d.screenshotAlt}
              width={2760}
              height={1494}
              sizes="(max-width: 58rem) calc(100vw - 2rem), 56rem"
              priority
            />
            <figcaption>
              <p className="dotline">
                <span>{d.shotPreview}</span>
                <span>{fill(d.shotBuild, { version: sourceVersion })}</span>
              </p>
              {/*
                The TUI header grammar: a `cw` chip and a dot chain. Each fact is
                its own translated unit — the separators are CSS punctuation, so
                nothing is concatenated around a token and no locale inherits an
                English joining word.
              */}
              <p
                className="paper-facts dotline"
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
                <span>{facts.license ?? "MIT"}</span>
              </p>
            </figcaption>
          </figure>

          <aside className="folio-chapter">
            <span className="folio-chapter-num">01 / {d.chapterTerminal}</span>
            <p className="folio-chapter-title">{d.chapterTerminalTitle}</p>
          </aside>
        </div>
      </section>

      {/* WHAT YOU GAIN — three ruled columns on paper. */}
      <section className="folio-section">
        <div className="product-container">
          <h2>{d.gainHeading}</h2>
          <p className="folio-section-lede">{d.gainLede}</p>
          <div className="folio-gain-grid">
            {d.gain.map(([title, body]) => (
              <div key={title}>
                <h3>{title}</h3>
                <p>{body}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* 02 — YOUR MODELS */}
      <section className="folio-section">
        <div className="product-container folio-chapter-grid">
          <div>
            <span className="folio-running-head">02 / {d.chapterModels}</span>
            <h2>{d.modelsHeading}</h2>
            <p className="folio-section-lede">{d.modelsBody}</p>
            <Link href={`/${locale}/models`} className="folio-link">
              {d.modelsLink}
            </Link>
          </div>
          <dl className="folio-fact-list">
            {d.modelsFacts.map(([kind, description]) => (
              <div key={kind}>
                <dt>{kind}</dt>
                <dd>{description}</dd>
              </div>
            ))}
            <div>
              <dt>Plan · Work · Operate</dt>
              <dd>Ask · Auto-Review · Full Access</dd>
            </div>
          </dl>
        </div>
      </section>

      {/* 03 — START */}
      <section className="product-start">
        <div className="product-container">
          <span className="folio-running-head">03 / {d.startHeading}</span>
          <h2>{d.startHeading}</h2>
          <p className="product-start-lede">{d.startLede}</p>
          <GettingStartedSteps locale={locale} />
          <div className="product-start-links">
            <Link href={`/${locale}/docs/guide`}>{d.startGuideLink}</Link>
            <Link href={`/${locale}/docs/vocabulary`}>{d.startVocabularyLink}</Link>
          </div>
        </div>
      </section>

      {/* THE WATERLINE. Everything below is one water column: the same
          component grammar, re-inked with the dark whale tokens by the
          shared below-the-waterline rule in globals.css. */}
      <div className="folio-waterline" aria-hidden="true">
        <Strata variant="band" />
      </div>
      <div className="ocean-column">
        {/* 04 — WHERE IT RUNS TODAY */}
        <section className="folio-availability">
          <div className="product-container">
            <span className="folio-running-head">04 / {d.chapterAccount}</span>
            <h2>{d.availabilityHeading}</h2>
            <p className="folio-section-lede">{d.availabilityLede}</p>
            <dl className="folio-availability-list">
              {d.availability.map(([surface, status, detail]) => (
                <div key={surface}>
                  <dt>{surface}</dt>
                  <dd>
                    <strong>{status}</strong>
                    {detail}
                  </dd>
                </div>
              ))}
            </dl>
            <p className="folio-availability-note">{d.availabilityNote}</p>
            <a href={APP_SIGNUP_URL} className="folio-link" data-usage="signup">
              {d.accountLink}
            </a>
          </div>
        </section>

        {/* Surfaces */}
        <section className="product-surfaces">
          <div className="product-container">
            <h2>{d.surfacesHeading}</h2>
            <div className="product-surface-list">
              {d.surfaces.map(([name, description]) => (
                <div key={name}>
                  <strong>{name}</strong>
                  <span>{description}</span>
                </div>
              ))}
            </div>
            <Link href={`/${locale}/runtime`}>{d.runtimeLink}</Link>
          </div>
        </section>

        {/* Install band — the composer plate. */}
        <section className="product-install-band">
          <div className="product-container product-install-grid">
            <h2>{d.installBandHeading}</h2>
            <div>
              {/* `❯` is a code-owned literal, like the install command it
                  prompts for — it is the product's glyph, not a sentence. */}
              <div className="product-composer">
                <span className="product-composer-prompt" aria-hidden>
                  ❯
                </span>
                <InstallCodeBlock
                  cmd={GETTING_STARTED_STEPS[0].commands[0]}
                  copyLabel={d.copy}
                  copiedLabel={d.copied}
                />
              </div>
              <p className="dotline">
                <span>GitHub Releases · {d.binaries}</span>
                <span>npm</span>
                <span>Cargo</span>
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
        <section className="product-community">
          <div className="product-container product-community-grid">
            <div>
              <h2>{d.communityHeading}</h2>
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
