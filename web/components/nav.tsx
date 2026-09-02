import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { getChrome } from "@/lib/i18n/dictionaries";
import { navLinks, REPO_URL, APP_LOGIN_URL, APP_SIGNUP_URL } from "@/lib/i18n/links";
import { fetchRepoStats, formatStars } from "@/lib/github";
import { getEnv } from "@/lib/kv";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";
import { NavLinks } from "./nav-links";
import { ThemeToggle } from "./theme-toggle";
import { Whale } from "./whale";

/** Masthead + primary nav — the Tideline topbar on the web. */
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

  return (
    <header className="site-nav paper-nav">
      <div className="site-nav-inner paper-nav-inner">
        <Link href={homeHref} className="site-wordmark paper-wordmark" aria-label={chrome.navHomeAria}>
          {/* The nav sits on the dark Tideline field on every route (the
              docs light sheet is scoped below it), so the mark is the white
              brand ink and the wordmark is the inverted trace. */}
          <div className="paper-wordmark-text">
            <Whale size={22} className="paper-wordmark-mark" />
            <img
              className="paper-wordmark-logo"
              src="/brand/wordmark-inverted.svg"
              alt=""
              width={142}
              height={20}
            />
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
          <span className="paper-auth" role="group" aria-label={chrome.authGroupAria}>
            <Link href={APP_LOGIN_URL} className="paper-auth-signin hidden lg:inline-flex">
              {chrome.authSignIn}
            </Link>
            <Link href={APP_SIGNUP_URL} className="paper-auth-register hidden lg:inline-flex">
              {chrome.authRegister}
            </Link>
          </span>
          <Link
            href={`/${locale}/install`}
            className="paper-install-cta hidden xl:inline-flex"
          >
            {chrome.installCta}
          </Link>
          <MobileMenu
            installHref={`/${locale}/install`}
            installLabel={chrome.installCta}
            signInHref={APP_LOGIN_URL}
            signInLabel={chrome.authSignIn}
            registerHref={APP_SIGNUP_URL}
            registerLabel={chrome.authRegister}
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
