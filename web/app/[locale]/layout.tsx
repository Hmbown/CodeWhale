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
    title: isZh ? "CodeWhale · DeepSeek V4 智能体运行框架" : "CodeWhale · DeepSeek V4 Agent Harness",
    description: isZh
      ? "面向 DeepSeek V4 和开放模型的本地 Agent 运行框架：自我、冲突法、本地工具、证据与恢复。"
      : "Local-first agent harness for DeepSeek V4 and open models, with operating identity, conflict law, local tools, evidence, and recovery.",
    metadataBase: new URL("https://codewhale.net"),
    openGraph: {
      title: "CodeWhale",
      description: isZh
        ? "本地 Agent 运行框架，内置自我、冲突法、本地工具、证据与恢复。"
        : "Local-first agent harness with operating identity, conflict law, local tools, evidence, and recovery.",
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
