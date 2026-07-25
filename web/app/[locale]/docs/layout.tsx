import Link from "next/link";
import { DocsSidebar } from "@/components/docs-sidebar";
import { Whale } from "@/components/whale";

/* ------------------------------------------------------------------ */
/*  Layout (Next.js App Router)                                        */
/* ------------------------------------------------------------------ */

export default async function DocsLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <div className="docs-theme docs-portal min-h-screen">
      <section className="docs-portal-hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container docs-portal-hero-inner">
          <div className="portal-mark">
            <Whale size={28} className="text-current" />
            <span>{isZh ? "Codewhale 文档" : "Codewhale documentation"}</span>
          </div>
          <h1>{isZh ? "查找准确的使用说明。" : "Find the guidance you need."}</h1>
          <p>
            {isZh
              ? "从安装和首次运行开始，或者直接查找模式、权限、工具、提供商、Fleet、MCP 与运行时 API。这些页面就是正式的产品文档；每页都链接仓库中的源文档，方便查阅完整细节。"
              : "Start with installation and first run, or go straight to modes, permissions, tools, providers, Fleet, MCP, and the Runtime API. These pages are the product documentation; each one cites the source documents in the repository for full detail."}
          </p>
          <div className="portal-actions">
            <Link href={`/${locale}/install`} className="portal-button portal-button-primary">
              {isZh ? "安装 Codewhale" : "Install Codewhale"}
            </Link>
            <Link
              href="https://github.com/Hmbown/CodeWhale/tree/main/docs"
              target="_blank"
              rel="noreferrer"
              className="portal-button portal-button-secondary"
            >
              {isZh ? "浏览源文档 ↗" : "Browse source docs ↗"}
            </Link>
          </div>
        </div>
      </section>

      <div className="portal-container docs-shell min-w-0">
        <article className="docs-content min-w-0">{children}</article>
        <DocsSidebar locale={locale} />
      </div>
    </div>
  );
}
