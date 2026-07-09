import Link from "next/link";
import type { Locale } from "@/lib/i18n/config";
import { FACTS } from "@/lib/facts.generated";
import { fetchRepoStats, formatStars } from "@/lib/github";
import { getEnv } from "@/lib/kv";
import { Seal } from "./seal";
import { Whale } from "./whale";
import { LocaleSwitcher } from "./locale-switcher";
import { MobileMenu } from "./mobile-menu";
import { NavLinks } from "./nav-links";
import { ThemeToggle } from "./theme-toggle";

const EN_LINKS = [
  { href: "/en/install", label: "Install", cn: "安装" },
  { href: "/en/constitution", label: "Constitution", cn: "宪法" },
  { href: "/en/docs", label: "Docs", cn: "文档" },
  { href: "/en/community", label: "Community", cn: "社区" },
  { href: "/en/faq", label: "FAQ", cn: "问答" },
];

const ZH_LINKS = [
  { href: "/zh/install", label: "安装", cn: "" },
  { href: "/zh/constitution", label: "宪法", cn: "" },
  { href: "/zh/docs", label: "文档", cn: "" },
  { href: "/zh/community", label: "社区", cn: "" },
  { href: "/zh/faq", label: "常见问题", cn: "" },
];

export async function Nav({ locale = "en" }: { locale?: Locale }) {
  const isZh = locale === "zh";
  const links = isZh ? ZH_LINKS : EN_LINKS;

  // Live star count — cached by fetchRepoStats (next.revalidate). Falls back
  // to a plain "GitHub" label when the API is unreachable at build time.
  let stars = 0;
  try {
    const env = await getEnv();
    stars = (await fetchRepoStats(env.GITHUB_TOKEN)).stars;
  } catch {
    /* keep fallback label */
  }

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
            <ThemeToggle isZh={isZh} />
            <span className="hidden md:inline">codewhale.net</span>
            <span className="tabular">{FACTS.version ? `v${FACTS.version}` : "v0.8.x"}</span>
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
            aria-label={isZh ? "GitHub 星标数" : "GitHub stars"}
          >
            <span>★ {stars > 0 ? formatStars(stars) : "GitHub"}</span>
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
