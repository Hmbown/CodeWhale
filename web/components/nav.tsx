import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { Seal } from "./seal";
import { Whale } from "./whale";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";

const EN_LINKS = [
  { href: "/install", label: "Install", cn: "安装" },
  { href: "/docs", label: "Docs", cn: "文档" },
  { href: "/feed", label: "Activity", cn: "动态" },
  { href: "/roadmap", label: "Roadmap", cn: "路线" },
  { href: "/contribute", label: "Contribute", cn: "参与" },
];

const ZH_LINKS = [
  { href: "/zh/install", label: "安装", cn: "" },
  { href: "/zh/docs", label: "文档", cn: "" },
  { href: "/zh/feed", label: "动态", cn: "" },
  { href: "/zh/roadmap", label: "路线图", cn: "" },
  { href: "/zh/contribute", label: "参与贡献", cn: "" },
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
            <span>{isZh ? `第 ${new Date().toISOString().slice(0, 10)} 期` : `第 ${new Date().toISOString().slice(0, 10)} 期`}</span>
            <span className="hidden sm:inline">· {isZh ? new Date().toLocaleDateString("zh-CN", { weekday: "long", month: "long", day: "numeric" }) : new Date().toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" })}</span>
          </div>
          <div className="flex items-center gap-4">
            <span className="hidden md:inline">deepseek-tui.com</span>
            <span className="inline-flex items-center gap-1.5">
              <span className="w-1.5 h-1.5 bg-jade rounded-full inline-block animate-pulse" />
              <span>{isZh ? "API · 在线" : "API · 在线"}</span>
            </span>
          </div>
        </div>
      </div>

      {/* main nav */}
      <div className="mx-auto max-w-[1400px] px-4 sm:px-6 py-3 flex items-center justify-between gap-3 sm:gap-6">
        <Link href={isZh ? "/zh" : "/"} className="flex items-center gap-3 group min-w-0">
          <Seal char="深" size="md" />
          <div className="leading-tight min-w-0">
            <div className="font-display text-[1.2rem] sm:text-[1.35rem] font-semibold tracking-crisp flex items-center gap-2 truncate">
              DeepSeek TUI
              <Whale size={20} className="text-indigo hidden sm:inline-block" />
            </div>
            <div className="font-cjk text-[0.65rem] sm:text-[0.7rem] text-ink-mute tracking-widest truncate">
              {isZh ? "深度求索 · 终端智能体" : "深度求索 · 终端智能体"}
            </div>
          </div>
        </Link>

        <nav className="hidden md:flex items-center gap-7">
          {links.map((l) => (
            <Link key={l.href} href={l.href} className="nav-link group">
              <span>{l.label}</span>
              {!isZh && "cn" in l && l.cn && (
                <span className="font-cjk text-[0.66rem] ml-1.5 text-ink-mute">{l.cn}</span>
              )}
            </Link>
          ))}
        </nav>

        <div className="flex items-center gap-2 sm:gap-3">
          <LocaleSwitcher current={locale} />
          <Link
            href="https://github.com/Hmbown/deepseek-tui"
            className="hidden sm:inline-flex items-center gap-2 px-3 py-1.5 hairline-t hairline-b hairline-l hairline-r font-mono text-[0.7rem] uppercase tracking-wider hover:bg-paper-deep transition-colors"
          >
            <span>★ GitHub</span>
          </Link>
          <Link
            href={isZh ? "/zh/install" : "/install"}
            className="hidden md:inline-flex items-center gap-2 px-3 py-1.5 bg-indigo text-paper font-mono text-[0.72rem] uppercase tracking-wider hover:bg-indigo-deep transition-colors"
          >
            {isZh ? "安装 →" : "Install →"}
          </Link>
          <MobileMenu
            installHref={isZh ? "/zh/install" : "/install"}
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
