import { Fragment } from "react";
import Image from "next/image";
import Link from "next/link";
import { GettingStartedSteps } from "@/components/getting-started-steps";
import { InstallCodeBlock } from "@/components/install-code-block";
import { Seal } from "@/components/seal";
import { TerminalPlayer } from "@/components/terminal-player";
import { Ticker } from "@/components/ticker";
import { Whale } from "@/components/whale";
import { getFacts } from "@/lib/facts";
import { fetchFeed } from "@/lib/github";
import { fill, getChrome, getHome, splitToken } from "@/lib/i18n/dictionaries";
import { REPO_ISSUES_URL, REPO_RELEASES_URL, REPO_URL } from "@/lib/i18n/links";
import { getEnv } from "@/lib/kv";
import type { FeedItem } from "@/lib/types";

// Revalidate against source-proven runtime facts without giving up static edge
// caching. `getFacts()` rejects legacy or older KV snapshots.
export const revalidate = 300;

/**
 * The newspaper-ocean homepage.
 *
 * Every visible string resolves through `getHome(locale)` / `getChrome(locale)`
 * — English, Chinese, and every other routed locale take the identical path,
 * with the English dictionary as the build-time-guaranteed fallback. The only
 * literals left in this file are code-owned per docs/VOICE.md: the product
 * control vocabulary (`Plan · Act · Operate`, `Ask · Auto-Review · Full
 * Access`, `TUI · exec · web · API`), the install command, `cargo test
 * --locked`, the receipt verbs, package-manager and mirror proper nouns, and
 * the screenshot path.
 */
export default async function HomePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const d = getHome(locale);
  const chrome = getChrome(locale);
  const facts = await getFacts();
  const sourceVersion = facts.version ?? "unknown";
  const publishedRelease = facts.latestPublishedRelease;
  const sourceIsPublished = publishedRelease?.version === sourceVersion;
  const providerCount = facts.providers.length;
  const providerRoutes = fill(d.providerRoutes, { count: providerCount });

  // The lede typesets the brand in its own span. Splitting on the {brand}
  // token keeps the sentence a single translated unit — no concatenation of
  // fragments around a variable, and a locale may place the brand anywhere.
  const ledeParts = splitToken(d.heroIntro, "brand");

  let feed: FeedItem[] = [];
  try {
    const env = await getEnv();
    feed = await fetchFeed(env.GITHUB_TOKEN, 12);
  } catch {
    /* ticker is optional chrome */
  }

  return (
    <div className="product-home paper-home">
      {/* HERO — newspaper split: claim + live terminal proof */}
      <section className="product-hero paper-hero">
        <div className="product-container product-hero-grid paper-hero-grid">
          <div className="product-hero-copy paper-hero-copy">
            <div className="mb-5">
              <span className="pill pill-hot">{d.kicker}</span>
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

            <p
              className="product-facts paper-facts"
              data-source-state={sourceIsPublished ? "published release" : "source candidate"}
              data-source-state-label={sourceIsPublished ? d.publishedRelease : d.figcaptionSourceCandidate}
            >
              {publishedRelease
                ? fill(d.latestRelease, { tag: publishedRelease.tag })
                : d.releaseUnavailable}{" "}
              <span>·</span>{" "}
              {`${sourceIsPublished ? d.currentSource : d.sourceCandidate} v${sourceVersion}: `}
              {providerRoutes} <span>·</span> {facts.license ?? "MIT"}
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
              src="/codewhale-tui.png"
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

      {/* Live repo ticker — industrial cyberpunk newspaper energy */}
      {feed.length > 0 ? (
        <Ticker
          items={feed}
          liveLabel={chrome.tickerLiveLabel}
          liveTag={chrome.tickerLiveTag}
        />
      ) : null}

      {/* Proof strip */}
      <section className="product-proof paper-proof">
        <div className="product-container product-proof-grid">
          <h2 className="font-display">{d.proofHeading}</h2>
          <p>{d.proofBody}</p>
        </div>
      </section>

      {/* See how it decides — constitution traces in terminal chrome */}
      <section className="paper-decides">
        <div className="product-container paper-decides-grid">
          <div>
            <div className="flex items-baseline gap-4 mb-3 hairline-b pb-3">
              <Seal char={d.sealDecides} size="sm" variant="indigo" />
              <div>
                <div className="eyebrow mb-1">{d.decidesEyebrow}</div>
                <h2 className="font-display text-2xl sm:text-3xl">{d.decidesHeading}</h2>
              </div>
            </div>
            <p className="paper-decides-lede">{d.decidesLede}</p>
          </div>
          <div>
            <TerminalPlayer
              locale={locale}
              traceLabel={chrome.traceLabel}
              tabsAria={chrome.traceTabsAria}
            />
          </div>
        </div>
      </section>

      {/* Workflow */}
      <section className="product-workflow paper-workflow">
        <div className="product-container">
          <div className="flex items-baseline gap-4 mb-6 hairline-b pb-4">
            <Seal char={d.sealWorkflow} size="sm" />
            <h2 className="font-display">{d.workflowHeading}</h2>
          </div>
          <ol className="product-workflow-steps">
            {d.workflow.map(([title, description], index) => (
              <li key={title}>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <h3>{title}</h3>
                <p>{description}</p>
              </li>
            ))}
          </ol>
          <div className="product-receipt" aria-label={d.receiptAria}>
            <span>$ codewhale exec &quot;fix the failing test&quot;</span>
            <span>inspect&nbsp;&nbsp; {d.receiptInspect}</span>
            <span>act&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp; {d.receiptAct}</span>
            <span>verify&nbsp;&nbsp;&nbsp; cargo test --locked</span>
            <strong>report&nbsp;&nbsp;&nbsp; {d.receiptReport}</strong>
          </div>
        </div>
      </section>

      {/* Getting started */}
      <section className="product-start paper-start">
        <div className="product-container">
          <div className="flex items-baseline gap-4 mb-4 hairline-b pb-4">
            <Seal char={d.sealStart} size="sm" />
            <h2 className="font-display">{d.startHeading}</h2>
          </div>
          <p className="product-start-lede">{d.startLede}</p>
          <GettingStartedSteps locale={locale} />
          <div className="product-start-links">
            <Link href={`/${locale}/docs/guide`}>{d.startGuideLink}</Link>
            <Link href={`/${locale}/docs/vocabulary`}>{d.startVocabularyLink}</Link>
          </div>
        </div>
      </section>

      {/* Boundaries */}
      <section className="product-boundaries paper-boundaries">
        <div className="product-container product-boundaries-grid">
          <div>
            <div className="flex items-baseline gap-4 mb-4">
              <Seal char={d.sealBoundaries} size="sm" />
              <h2 className="font-display">
                {d.boundariesHeadingA}
                <br />
                <span>{d.boundariesHeadingB}</span>
              </h2>
            </div>
            <p>{d.boundariesBody}</p>
          </div>
          <dl className="product-boundary-list">
            <div>
              <dt>{providerRoutes}</dt>
              <dd>{d.hostedGatewayLocal}</dd>
            </div>
            <div>
              <dt>Plan · Act · Operate</dt>
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

      {/* Surfaces */}
      <section className="product-surfaces paper-surfaces">
        <div className="product-container">
          <div className="flex items-baseline gap-4 mb-6 hairline-b pb-4">
            <Seal char={d.sealSurfaces} size="sm" />
            <h2 className="font-display">{d.surfacesHeading}</h2>
          </div>
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

      {/* Install band */}
      <section className="product-install-band paper-install-band">
        <div className="product-container product-install-grid">
          <h2 className="font-display">{d.installBandHeading}</h2>
          <div>
            <InstallCodeBlock
              cmd="npm install -g codewhale"
              copyLabel={d.copy}
              copiedLabel={d.copied}
            />
            <p>
              Cargo · {d.binaries} · Docker · Nix · Windows · Android / Termux · {d.chinaMirrors}
            </p>
            <Link href={`/${locale}/install`}>{d.installGuideLink}</Link>
          </div>
        </div>
      </section>

      {/* Community */}
      <section className="product-community paper-community">
        <div className="product-container product-community-grid">
          <div className="product-community-illustration" aria-hidden="true">
            <Seal char={d.sealCommunity} size="lg" />
          </div>
          <div>
            <h2 className="font-display">{d.communityHeading}</h2>
            <p>{d.communityBody}</p>
          </div>
          <nav aria-label={d.communityLinksAria}>
            <a href={REPO_URL}>GitHub</a>
            <a href={REPO_ISSUES_URL}>Issues</a>
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
  );
}
