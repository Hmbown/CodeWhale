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
      ? "DeepSeek V4 的最强智能体运行框架。宪政层级、结构化信任、验证与恢复——让模型持续工作并不断进步的规则、工具和反馈循环。国际开源社区，递归自改进。"
      : "The most agentic harness for DeepSeek V4. Constitutional hierarchy, structured trust, verification, and recovery — rules, tools, and feedback loops that help the model keep working. An international open source community building a recursive, self-improving harness.",
    metadataBase: new URL("https://codewhale.net"),
    openGraph: {
      title: "CodeWhale",
      description: isZh
        ? "DeepSeek V4 的最强智能体运行框架。宪政层级、结构化信任、验证与恢复。"
        : "The most agentic harness for DeepSeek V4. Constitutional hierarchy, structured trust, verification, and recovery.",
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
