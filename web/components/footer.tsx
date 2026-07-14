import Link from "next/link";
import { GITEE_ENABLED, type Locale } from "@/lib/i18n/config";
import { Whale } from "./whale";

export function Footer({ locale = "en" }: { locale?: Locale }) {
  const isZh = locale === "zh";

  const product = isZh
    ? [
        { label: "文档", href: "/zh/docs" },
        { label: "安装", href: "/zh/install" },
        { label: "模型", href: "/zh/models" },
        { label: "运行时", href: "/zh/runtime" },
      ]
    : [
        { label: "Docs", href: "/en/docs" },
        { label: "Install", href: "/en/install" },
        { label: "Models", href: "/en/models" },
        { label: "Runtime", href: "/en/runtime" },
      ];

  const project = isZh
    ? [
        { label: "GitHub", href: "https://github.com/Hmbown/CodeWhale" },
        { label: "议题", href: "https://github.com/Hmbown/CodeWhale/issues" },
        { label: "参与贡献", href: "/zh/contribute" },
        { label: "MIT 许可证", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" },
      ]
    : [
        { label: "GitHub", href: "https://github.com/Hmbown/CodeWhale" },
        { label: "Issues", href: "https://github.com/Hmbown/CodeWhale/issues" },
        { label: "Contribute", href: "/en/contribute" },
        { label: "MIT license", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" },
      ];

  return (
    <footer className="site-footer">
      <div className="site-footer-main">
        <div className="site-footer-brand">
          <Link href={isZh ? "/zh" : "/en"} className="site-wordmark site-wordmark-footer">
            <Whale size={31} className="text-current" />
            <span>Codewhale</span>
          </Link>
          <p>
            {isZh
              ? "Codewhale 开源运行时的文档、源码与社区入口。"
              : "Documentation, source, and community for the open-source Codewhale runtime."}
          </p>
        </div>

        <div className="site-footer-links">
          <div>
            <span>{isZh ? "产品" : "Product"}</span>
            {product.map((item) => <Link key={item.href} href={item.href}>{item.label}</Link>)}
          </div>
          <div>
            <span>{isZh ? "项目" : "Project"}</span>
            {project.map((item) => <Link key={item.href} href={item.href}>{item.label}</Link>)}
          </div>
        </div>
      </div>

      <div className="site-footer-meta">
        <p>
          {isZh ? "官方源码与发布：" : "Canonical source and releases: "}
          <a href="https://github.com/Hmbown/CodeWhale">github.com/Hmbown/CodeWhale</a>
        </p>
        <div>
          {GITEE_ENABLED && <a href="https://gitee.com/Hmbown/CodeWhale">Gitee</a>}
          <a href="https://cnb.cool/codewhale.net/codewhale">CNB</a>
          <a href="https://npmmirror.com/package/codewhale">npmmirror</a>
          <a href="mailto:hmbown@gmail.com">Security</a>
        </div>
        <span>© {new Date().getFullYear()} Codewhale</span>
      </div>
    </footer>
  );
}
