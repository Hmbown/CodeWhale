import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { FACTS } from "@/lib/facts.generated";
import { fill, getChrome } from "@/lib/i18n/dictionaries";
import { navLinks, REPO_URL } from "@/lib/i18n/links";
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
            ★ {stars > 0 ? formatStars(stars) : chrome.githubFallback}
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
          />
        </div>
      </div>
    </header>
  );
}
