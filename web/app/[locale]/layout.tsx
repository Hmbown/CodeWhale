import type { Metadata } from "next";
import { Fraunces, IBM_Plex_Sans, JetBrains_Mono, Noto_Serif_SC } from "next/font/google";
import { Nav } from "@/components/nav";
import { Footer } from "@/components/footer";
import { locales, type Locale } from "@/lib/i18n/config";
import { buildPageMetadata } from "@/lib/page-meta";
import "../globals.css";

const display = Fraunces({
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
      ? "CodeWhale — 适配任意模型的终端编程智能体，开放模型优先"
      : "CodeWhale — the terminal coding agent for any model, open models first",
    description: isZh
      ? "开源终端编程智能体：适配任意模型，开放模型优先。从 DeepSeek、本地 vLLM/Ollama 到原生 Claude 与 OpenAI，内置审批制工具、沙箱隔离与 /restore 回滚。"
      : "Open-source terminal coding agent for any model, open models first: broad provider support from DeepSeek and local vLLM/Ollama to native Claude and OpenAI, with approval-gated tools, sandboxing, and /restore rollback.",
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
