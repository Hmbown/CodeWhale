import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { FACTS } from "@/lib/facts.generated";
import { fill, getChrome } from "@/lib/i18n/dictionaries";
import { navLinks, REPO_URL, DISCORD_URL } from "@/lib/i18n/links";
import { fetchRepoStats, formatStars } from "@/lib/github";
import { getEnv } from "@/lib/kv";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";
import { NavLinks } from "./nav-links";
import { Seal } from "./seal";
import { ThemeToggle } from "./theme-toggle";
import { Whale } from "./whale";

/**
 * Newspaper masthead + primary nav.
 *
 * One dictionary path for every routed locale: labels, the issue strip, the
 * wordmark seal and strapline, the star-badge aria, and the theme/menu
 * controls all come from `getChrome(locale)`. No `isZh` branch, and the
 * masthead weekday uses the locale's own Intl tag rather than en-US.
 */
export async function Nav({ locale = "en" }: { locale?: Locale }) {
  const chrome = getChrome(locale);
  const links = navLinks(locale, chrome);
  const homeHref = `/${locale}`;

  // Live star count — cached by fetchRepoStats. Falls back to a plain GitHub
  // label when the API is unreachable at build time.
  let stars = 0;
  try {
    const env = await getEnv();
    stars = (await fetchRepoStats(env.GITHUB_TOKEN)).stars;
  } catch {
    /* keep fallback label */
  }

  const now = new Date();
  const issueDate = now.toISOString().slice(0, 10);
  const weekday = now.toLocaleDateString(chrome.dateLocale, {
    weekday: "long",
    month: "long",
    day: "numeric",
  });
  const versionLabel = FACTS.version ? `v${FACTS.version}` : "v0.9.x";

  return (
    <header className="site-nav paper-nav">
      {/* Issue / build strip — the newspaper masthead people loved */}
      <div className="paper-issue-bar">
        <div className="paper-issue-inner">
          <div className="paper-issue-left">
            <span>{fill(chrome.issueLabel, { date: issueDate })}</span>
            <span className="paper-issue-sep" aria-hidden>
              ·
            </span>
            <span className="hidden sm:inline">{weekday}</span>
          </div>
          <div className="paper-issue-right">
            <span className="hidden md:inline">codewhale.net</span>
            <span className="tabular">{versionLabel}</span>
          </div>
        </div>
      </div>

      <div className="site-nav-inner paper-nav-inner">
        <Link href={homeHref} className="site-wordmark paper-wordmark" aria-label={chrome.navHomeAria}>
          <Seal char={chrome.wordmarkSeal} size="md" />
          <div className="paper-wordmark-text">
            <span className="paper-wordmark-name">
              Codewhale
              <Whale size={18} className="paper-wordmark-whale" />
            </span>
            <span className="paper-wordmark-tag">{chrome.wordmarkTag}</span>
          </div>
        </Link>

        <NavLinks links={links} primaryAria={chrome.navPrimaryAria} />

        <div className="site-nav-actions">
          <ThemeToggle
            autoLabel={chrome.themeAuto}
            lightLabel={chrome.themeLight}
            darkLabel={chrome.themeDark}
            ariaTemplate={chrome.themeAria}
            titleLabel={chrome.themeTitle}
          />
          <LocaleSwitcher current={locale} />
          <Link
            href={REPO_URL}
            className="site-github-link paper-star-badge"
            aria-label={chrome.starsAria}
          >
            <svg viewBox="0 0 16 16" aria-hidden fill="currentColor" className="brand-mark"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>
            ★ {stars > 0 ? formatStars(stars) : chrome.githubFallback}
          </Link>
          <Link
            href={DISCORD_URL}
            className="site-discord-link paper-discord-badge"
            aria-label="Discord"
          >
            <svg viewBox="0 0 71 55" aria-hidden fill="currentColor" className="brand-mark"><path d="M60.1 4.9C55.6 2.8 50.7 1.3 45.6.4c-.6 1.1-1.3 2.6-1.8 3.8-5.4-.8-10.8-.8-16.1 0-.5-1.2-1.2-2.7-1.8-3.8-5.1.9-10 2.4-14.5 4.5C2.6 18.2-.4 31.2 1 44c6.1 4.6 12.1 7.4 17.9 9.3 1.4-1.9 2.7-4 3.8-6.2-2.1-.8-4.1-1.8-6-3 .5-.4 1-.8 1.5-1.2 11.5 5.4 24 5.4 35.4 0 .5.4 1 .8 1.5 1.2-1.9 1.2-3.9 2.2-6 3 1.1 2.2 2.4 4.3 3.8 6.2 5.8-1.9 11.8-4.7 17.9-9.3 1.7-15-2.9-27.9-10.7-39.1zM23.7 36.3c-3.5 0-6.4-3.2-6.4-7.2s2.8-7.2 6.4-7.2 6.5 3.2 6.4 7.2c0 4-2.9 7.2-6.4 7.2zm23.4 0c-3.5 0-6.4-3.2-6.4-7.2s2.8-7.2 6.4-7.2c3.5 0 6.5 3.2 6.4 7.2 0 4-2.9 7.2-6.4 7.2z"/></svg>
          </Link>
          <Link
            href={`/${locale}/install`}
            className="paper-install-cta hidden md:inline-flex"
          >
            {chrome.installCta}
          </Link>
          <MobileMenu
            installHref={`/${locale}/install`}
            installLabel={chrome.installCta}
            links={links}
            openLabel={chrome.menuOpen}
            closeLabel={chrome.menuClose}
            navAria={chrome.navPrimaryAria}
          />
        </div>
      </div>
    </header>
  );
}
