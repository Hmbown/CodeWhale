import Link from "next/link";
import { GITEE_ENABLED, type Locale } from "@/lib/i18n/config";
import { getChrome } from "@/lib/i18n/dictionaries";
import {
  footerLegalLinks,
  footerProductLinks,
  footerProjectLinks,
  REPO_RELEASES_URL,
  REPO_URL,
} from "@/lib/i18n/links";
import { Whale } from "./whale";

/**
 * Site footer. One dictionary path for every routed locale — the previous
 * en / zh / dictionary triple branch is gone, and the Product column now
 * carries the full six-link set everywhere (footerGuide and footerFaq exist
 * in ChromeDict as of #4934).
 */
export function Footer({ locale = "en" }: { locale?: Locale }) {
  const chrome = getChrome(locale);
  const homeHref = `/${locale}`;
  const product = footerProductLinks(locale, chrome);
  const project = footerProjectLinks(locale, chrome);
  const legal = footerLegalLinks(locale);

  return (
    <footer className="site-footer">
      <div className="site-footer-main">
        <div className="site-footer-brand">
          <Link href={homeHref} className="site-wordmark site-wordmark-footer">
            <Whale size={31} caustic />
            <span>Codewhale</span>
          </Link>
          <p>{chrome.footerTagline}</p>
        </div>

        <div className="site-footer-links">
          <div>
            <span>{chrome.footerProduct}</span>
            {product.map((item) => <Link key={item.href} href={item.href}>{item.label}</Link>)}
          </div>
          <div>
            <span>{chrome.footerProject}</span>
            {project.map((item) => <Link key={item.href} href={item.href}>{item.label}</Link>)}
          </div>
        </div>
      </div>

      <div className="site-footer-meta">
        <p>
          {chrome.footerCanonicalSource}
          <a href={REPO_URL}>github.com/Hmbown/CodeWhale</a>
          {chrome.footerReleases}
          <a href={REPO_RELEASES_URL}>{chrome.footerReleasesLink}</a>
        </p>
        <div>
          {legal.map((item) => <Link key={item.href} href={item.href}>{item.label}</Link>)}
          {GITEE_ENABLED && <a href="https://gitee.com/Hmbown/CodeWhale">Gitee</a>}
          <a href="https://cnb.cool/codewhale.net/codewhale">CNB</a>
          <a href="https://npmmirror.com/package/codewhale">npmmirror</a>
          <a href="mailto:hmbown@gmail.com">{chrome.footerSecurity}</a>
        </div>
        <span>© {new Date().getFullYear()} Codewhale</span>
      </div>
    </footer>
  );
}
