import Link from "next/link";
import { buildPageMetadata } from "@/lib/page-meta";
import { LEGAL_UPDATED, TERMS_SECTIONS } from "@/lib/legal-copy";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/legal/terms",
    locale,
    title: isZh ? "服务条款 · Codewhale" : "Terms of service · Codewhale",
    description: isZh
      ? "Shannon Labs 产品 Codewhale 的服务条款。"
      : "Terms that govern your use of Codewhale, a Shannon Labs product.",
  });
}

export default async function TermsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return (
    <div className="portal-home">
      <article className="legal-doc">
        <p className="legal-doc-kicker">{isZh ? "法律" : "Legal"}</p>
        <h1>{isZh ? "服务条款" : "Terms of service"}</h1>
        <p className="legal-doc-updated">
          {isZh ? "生效并最近更新于" : "Effective and last updated"} {LEGAL_UPDATED}
          {isZh ? "。以下为具有约束力的英文文本。" : "."}
        </p>
        <p>
          These terms govern your use of Codewhale, a Shannon Labs product. By creating an
          account or using the service, you agree to them.
        </p>
        {TERMS_SECTIONS.map((section) => (
          <section key={section.title}>
            <h2>{section.title}</h2>
            <p>{section.body}</p>
          </section>
        ))}
        <p className="legal-doc-nav">
          <Link href={`/${locale}/legal/privacy`}>{isZh ? "隐私政策" : "Privacy policy"}</Link>
          <Link href={`/${locale}/pricing`}>{isZh ? "价格" : "Pricing"}</Link>
          <Link href={`/${locale}`}>{isZh ? "返回首页" : "Back home"}</Link>
        </p>
      </article>
    </div>
  );
}
