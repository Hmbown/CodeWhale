import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";
import { NavLinks } from "./nav-links";
import { ThemeToggle } from "./theme-toggle";
import { Whale } from "./whale";

const EN_LINKS = [
  { href: "/en/docs", label: "Docs" },
  { href: "/en/install", label: "Install" },
  { href: "/en/community", label: "Community" },
  { href: "/en/contribute", label: "Contribute" },
];

const ZH_LINKS = [
  { href: "/zh/docs", label: "文档" },
  { href: "/zh/install", label: "安装" },
  { href: "/zh/community", label: "社区" },
  { href: "/zh/contribute", label: "贡献" },
];

export function Nav({ locale = "en" }: { locale?: Locale }) {
  const isZh = locale === "zh";
  const links = isZh ? ZH_LINKS : EN_LINKS;

  return (
    <header className="site-nav">
      <div className="site-nav-inner">
        <Link href={isZh ? "/zh" : "/en"} className="site-wordmark" aria-label="Codewhale home">
          <Whale size={31} className="text-current" />
          <span>Codewhale</span>
        </Link>

        <NavLinks links={links} isZh={isZh} />

        <div className="site-nav-actions">
          <ThemeToggle isZh={isZh} />
          <LocaleSwitcher current={locale} />
          <Link href="https://github.com/Hmbown/CodeWhale" className="site-github-link">
            GitHub
          </Link>
          <MobileMenu
            installHref={isZh ? "/zh/install" : "/en/install"}
            installLabel={isZh ? "安装 →" : "Install →"}
            links={links}
          />
        </div>
      </div>
    </header>
  );
}
