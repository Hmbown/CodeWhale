import type { Metadata } from "next";
import { Nav } from "@/components/nav";
import { Footer } from "@/components/footer";
import { locales, type Locale } from "@/lib/i18n/config";

export function generateStaticParams() {
  return locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }): Promise<Metadata> {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "CodeWhale · 终端原生编程智能体" : "CodeWhale · 深度求索 终端",
    description: isZh
      ? "面向开源模型的终端编程智能体。DeepSeek V4 为一级模型。支持 100 万 token 上下文、MCP 协议、沙箱执行。"
      : "Terminal-native coding agent for open-source and open-weight models across providers. DeepSeek V4 is first-class. Community site for installation, docs, roadmap, and live activity.",
    metadataBase: new URL("https://codewhale.net"),
    openGraph: {
      title: isZh ? "CodeWhale · 终端原生编程智能体" : "CodeWhale",
      description: isZh
        ? "面向开源模型的终端编程智能体。"
        : "Terminal-native coding agent for open-source and open-weight models across providers.",
      url: "https://codewhale.net",
      siteName: "CodeWhale",
      type: "website",
    },
    twitter: { card: "summary_large_image" },
    alternates: {
      languages: {
        en: "/en",
        zh: "/zh",
      },
    },
  };
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
    <>
      <Nav locale={locale as Locale} />
      <main>{children}</main>
      <Footer locale={locale as Locale} />
    </>
  );
}
