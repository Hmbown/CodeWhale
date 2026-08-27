import Link from "next/link";
import { buildPageMetadata } from "@/lib/page-meta";
import { LEGAL_UPDATED, PRIVACY_SECTIONS } from "@/lib/legal-copy";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/legal/privacy",
    locale,
    title: isZh ? "隐私政策 · Codewhale" : "Privacy policy · Codewhale",
    description: isZh
      ? "Shannon Labs 如何在你使用 Codewhale 时处理信息。"
      : "How Shannon Labs handles information when you use Codewhale.",
  });
}

export default async function PrivacyPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return (
    <div className="portal-home">
      <article className="legal-doc">
        <p className="legal-doc-kicker">{isZh ? "法律" : "Legal"}</p>
        <h1>{isZh ? "隐私政策" : "Privacy policy"}</h1>
        <p className="legal-doc-updated">
          {isZh ? "生效并最近更新于" : "Effective and last updated"} {LEGAL_UPDATED}
          {isZh ? "。以下为具有约束力的英文文本。" : "."}
        </p>
        <p>This policy explains how Shannon Labs handles information when you use Codewhale.</p>
        {PRIVACY_SECTIONS.map((section) => (
          <section key={section.title}>
            <h2>{section.title}</h2>
            <p>{section.body}</p>
          </section>
        ))}
        <p className="legal-doc-nav">
          <Link href={`/${locale}/legal/terms`}>{isZh ? "服务条款" : "Terms of service"}</Link>
          <Link href={`/${locale}/pricing`}>{isZh ? "价格" : "Pricing"}</Link>
          <Link href={`/${locale}`}>{isZh ? "返回首页" : "Back home"}</Link>
        </p>
      </article>
    </div>
  );
}
