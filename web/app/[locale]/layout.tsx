import type { Metadata } from "next";
import { IBM_Plex_Sans, JetBrains_Mono, Noto_Serif_SC, Space_Grotesk } from "next/font/google";
import { Nav } from "@/components/nav";
import { Footer } from "@/components/footer";
import { locales, type Locale } from "@/lib/i18n/config";
import { buildPageMetadata } from "@/lib/page-meta";
import "../globals.css";

const display = Space_Grotesk({
  subsets: ["latin"],
  weight: ["400", "500", "600", "700"],
  variable: "--font-display",
  display: "swap",
});

const body = IBM_Plex_Sans({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-body",
  display: "swap",
});

const mono = JetBrains_Mono({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-mono",
  display: "swap",
});

// Noto Serif SC is heavy; load only what we need for decorative anchors.
const cjk = Noto_Serif_SC({
  subsets: ["latin"],
  weight: ["400", "700"],
  variable: "--font-cjk",
  display: "swap",
  preload: false,
});

export function generateStaticParams() {
  return locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }): Promise<Metadata> {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/",
    locale,
    title: isZh
      ? "Codewhale 文档与开源运行时"
      : "Codewhale documentation and open-source runtime",
    description: isZh
      ? "安装 Codewhale，并查找有关模式、权限、工具、提供商、配置、运行时 API 和社区贡献的准确文档。"
      : "Install Codewhale and find precise documentation for modes, permissions, tools, providers, configuration, the runtime API, and community contributions.",
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

  return (
    <html
      lang={locale === "zh" ? "zh" : "en"}
      className={`${display.variable} ${body.variable} ${mono.variable} ${cjk.variable}`}
      suppressHydrationWarning
    >
      <body>
        {/* Apply the persisted docs theme before paint so there is no flash.
            "auto" leaves data-theme unset and defers to prefers-color-scheme. */}
        <script
          dangerouslySetInnerHTML={{
            __html:
              "(function(){try{var t=localStorage.getItem('cw-theme');if(t==='light'||t==='dark'){document.documentElement.setAttribute('data-theme',t);}}catch(e){}})();",
          }}
        />
        <Nav locale={locale as Locale} />
        <main>{children}</main>
        <Footer locale={locale as Locale} />
      </body>
    </html>
  );
}
