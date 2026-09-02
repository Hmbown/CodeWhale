import type { Metadata } from "next";
import { IBM_Plex_Sans, IBM_Plex_Sans_Condensed, JetBrains_Mono } from "next/font/google";
import { Nav } from "@/components/nav";
import { Footer } from "@/components/footer";
import { localeDirection, locales, type Locale } from "@/lib/i18n/config";
import { getChrome, getHome } from "@/lib/i18n/dictionaries";
import { serializeJsonLd } from "@/lib/json-ld";
import { buildPageMetadata } from "@/lib/page-meta";
import { buildSiteJsonLd } from "@/lib/site-schema";
import "../globals.css";

// Display voice is IBM Plex Sans Condensed (condensed sibling of the body face).
// Display/CJK stacks resolve in globals.css.
const body = IBM_Plex_Sans({
  subsets: ["latin", "cyrillic", "vietnamese"],
  weight: ["400", "500", "600"],
  variable: "--font-body",
  display: "swap",
});

const mono = JetBrains_Mono({
  subsets: ["latin", "cyrillic"],
  weight: ["400", "500", "600"],
  variable: "--font-mono",
  display: "swap",
});

const display = IBM_Plex_Sans_Condensed({
  subsets: ["latin"],
  weight: ["500", "600"],
  variable: "--font-display",
  display: "swap",
});

export function generateStaticParams() {
  return locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }): Promise<Metadata> {
  const { locale } = await params;
  const home = getHome(locale);
  return buildPageMetadata({
    path: "/",
    locale,
    title: home.metaTitle,
    description: home.metaDescription,
  });
}

export default async function LocaleLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const chrome = getChrome(locale);
  // RTL locales (e.g. ar) set the document direction from the canonical
  // registry so the browser handles bidirectional layout from the root.
  const dir = localeDirection(locale);
  const siteJsonLd = buildSiteJsonLd(locale);

  return (
    <html
      lang={locale}
      dir={dir}
      className={`${body.variable} ${mono.variable} ${display.variable}`}
      suppressHydrationWarning
    >
      <body>
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: serializeJsonLd(siteJsonLd) }}
        />
        {/* Apply the persisted docs theme before paint so there is no flash.
            The site default is the Tideline dark field; only an explicit
            "light" choice re-themes the docs sheet. */}
        <script
          dangerouslySetInnerHTML={{
            __html:
              "(function(){try{var t=localStorage.getItem('cw-theme');if(t==='light'||t==='dark'){document.documentElement.setAttribute('data-theme',t);}}catch(e){}})();",
          }}
        />
        <a href="#main-content" className="skip-link">
          {chrome.skipToContent}
        </a>
        <Nav locale={locale as Locale} />
        <main id="main-content">{children}</main>
        <Footer locale={locale as Locale} />
      </body>
    </html>
  );
}
