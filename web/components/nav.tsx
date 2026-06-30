import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { Seal } from "./seal";
import { Whale } from "./whale";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";
import { NavLinks } from "./nav-links";

const EN_LINKS = [
  { href: "/en/install", label: "Install", cn: "安装" },
  { href: "/en/runtime", label: "Runtime", cn: "集成" },
  { href: "/en/docs", label: "Docs", cn: "文档" },
  { href: "/en/feed", label: "Activity", cn: "动态" },
  { href: "/en/digest", label: "Community Digest", cn: "社区摘要" },
  { href: "/en/roadmap", label: "Roadmap", cn: "路线" },
  { href: "/en/faq", label: "FAQ", cn: "问答" },
  { href: "/en/contribute", label: "Contribute", cn: "参与" },
  { href: "https://github.com/Hmbown/CodeWhale", label: "GitHub", cn: "源码" },
];

const ZH_LINKS = [
  { href: "/zh/install", label: "安装", cn: "" },
  { href: "/zh/runtime", label: "集成", cn: "" },
  { href: "/zh/docs", label: "文档", cn: "" },
  { href: "/zh/feed", label: "动态", cn: "" },
  { href: "/zh/digest", label: "社区摘要", cn: "" },
  { href: "/zh/roadmap", label: "路线图", cn: "" },
  { href: "/zh/faq", label: "常见问题", cn: "" },
  { href: "/zh/contribute", label: "参与贡献", cn: "" },
  { href: "https://github.com/Hmbown/CodeWhale", label: "GitHub", cn: "" },
];

export function Nav({ locale = "en" }: { locale?: Locale }) {
  const isZh = locale === "zh";
  const links = isZh ? ZH_LINKS : EN_LINKS;

  return (
    <header className="hairline-b bg-paper/85 backdrop-blur sticky top-0 z-30">
      {/* date / build strip */}
      <div className="hairline-b">
        <div className="mx-auto max-w-[1400px] px-6 py-1.5 flex items-center justify-between text-[0.66rem] font-mono uppercase tracking-[0.18em] text-ink-mute">
          <div className="flex items-center gap-4">
            <span>{isZh ? `第 ${new Date().toISOString().slice(0, 10)} 期` : `Issue ${new Date().toISOString().slice(0, 10)}`}</span>
            <span className="hidden sm:inline">· {isZh ? new Date().toLocaleDateString("zh-CN", { weekday: "long", month: "long", day: "numeric" }) : new Date().toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" })}</span>
          </div>
          <div className="flex items-center gap-4">
            <span className="hidden md:inline">codewhale.net</span>
            <span className="tabular">v0.8.x</span>
          </div>
        </div>
      </div>

      {/* main nav */}
      <div className="mx-auto max-w-[1400px] px-4 sm:px-6 py-3 flex items-center justify-between gap-3 sm:gap-6">
        <Link href={isZh ? "/zh" : "/en"} className="flex items-center gap-3 group min-w-0">
          <Seal char="深" size="md" />
          <div className="leading-tight min-w-0">
            <div className="font-display text-[1.2rem] sm:text-[1.35rem] font-semibold tracking-crisp flex items-center gap-2 truncate">
              CodeWhale
              <Whale size={20} className="text-indigo hidden sm:inline-block" />
            </div>
            <div className="font-cjk text-[0.65rem] sm:text-[0.7rem] text-ink-mute tracking-widest truncate">
              {isZh ? "任何模型 · 开源模型优先" : "any model, open models first"}
            </div>
          </div>
        </Link>

        <NavLinks links={links} isZh={isZh} />

        <div className="flex items-center gap-2 sm:gap-3">
          <LocaleSwitcher current={locale} />
          <Link
            href="https://github.com/Hmbown/CodeWhale"
            className="hidden sm:inline-flex items-center gap-2 px-3 py-1.5 hairline-t hairline-b hairline-l hairline-r font-mono text-[0.7rem] uppercase tracking-wider hover:bg-paper-deep transition-colors"
          >
            <span>★ GitHub</span>
          </Link>
          <Link
            href={isZh ? "/zh/install" : "/en/install"}
            className="hidden md:inline-flex items-center gap-2 px-3 py-1.5 bg-indigo text-paper font-mono text-[0.72rem] uppercase tracking-wider hover:bg-indigo-deep transition-colors"
          >
            {isZh ? "安装 →" : "Install →"}
          </Link>
          <MobileMenu
            installHref={isZh ? "/zh/install" : "/en/install"}
            installLabel={isZh ? "安装 →" : "Install →"}
            links={links.map((l) => ({
              href: l.href,
              label: l.label,
              cn: !isZh && "cn" in l ? l.cn : undefined,
            }))}
          />
        </div>
      </div>
    </header>
  );
}
