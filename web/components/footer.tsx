import Link from "next/link";
import { GITEE_ENABLED, type Locale } from "@/lib/i18n/config";
import { Seal } from "./seal";

const EN_COLS = [
  {
    title: "Product",
    cn: "产品",
    items: [
      { label: "Install", href: "/install" },
      { label: "Documentation", href: "/docs" },
      { label: "Roadmap", href: "/roadmap" },
      { label: "Releases", href: "https://github.com/Hmbown/deepseek-tui/releases" },
    ],
  },
  {
    title: "Community",
    cn: "社区",
    items: [
      { label: "Issues", href: "https://github.com/Hmbown/deepseek-tui/issues" },
      { label: "Pull Requests", href: "https://github.com/Hmbown/deepseek-tui/pulls" },
      { label: "Discussions", href: "https://github.com/Hmbown/deepseek-tui/discussions" },
      { label: "Contribute", href: "/contribute" },
      { label: "Support DeepSeek TUI", href: "https://buymeacoffee.com/hmbown" },
    ],
  },
  {
    title: "Resources",
    cn: "资源",
    items: [
      { label: "Activity Feed", href: "/feed" },
      { label: "Code of Conduct", href: "https://github.com/Hmbown/deepseek-tui/blob/main/CODE_OF_CONDUCT.md" },
      { label: "Security", href: "https://github.com/Hmbown/deepseek-tui/blob/main/SECURITY.md" },
      { label: "License (MIT)", href: "https://github.com/Hmbown/deepseek-tui/blob/main/LICENSE" },
    ],
  },
];

const ZH_COLS = [
  {
    title: "产品",
    items: [
      { label: "安装指南", href: "/zh/install" },
      { label: "使用文档", href: "/zh/docs" },
      { label: "路线图", href: "/zh/roadmap" },
      { label: "版本发布", href: "https://github.com/Hmbown/deepseek-tui/releases" },
    ],
  },
  {
    title: "社区",
    items: [
      { label: "议题", href: "https://github.com/Hmbown/deepseek-tui/issues" },
      { label: "合并请求", href: "https://github.com/Hmbown/deepseek-tui/pulls" },
      { label: "讨论区", href: "https://github.com/Hmbown/deepseek-tui/discussions" },
      { label: "参与贡献", href: "/zh/contribute" },
      { label: "支持 DeepSeek TUI", href: "https://buymeacoffee.com/hmbown" },
    ],
  },
  {
    title: "资源",
    items: [
      { label: "活动动态", href: "/zh/feed" },
      { label: "行为准则", href: "https://github.com/Hmbown/deepseek-tui/blob/main/CODE_OF_CONDUCT.md" },
      { label: "安全策略", href: "https://github.com/Hmbown/deepseek-tui/blob/main/SECURITY.md" },
      { label: "MIT 许可证", href: "https://github.com/Hmbown/deepseek-tui/blob/main/LICENSE" },
    ],
  },
];

export function Footer({ locale = "en" }: { locale?: Locale }) {
  const isZh = locale === "zh";
  const cols = isZh ? ZH_COLS : EN_COLS;

  return (
    <footer className="hairline-t mt-24 bg-paper-deep">
      <div className="mx-auto max-w-[1400px] px-6 py-12 grid grid-cols-2 md:grid-cols-5 gap-10">
        <div className="col-span-2 md:col-span-2 space-y-4">
          <div className="flex items-center gap-3">
            <Seal char="深" size="md" />
            <div>
              <div className="font-display text-xl font-semibold">DeepSeek TUI</div>
              <div className="font-cjk text-[0.7rem] text-ink-mute tracking-widest">
                {isZh ? "深度求索 · 终端智能体" : "深度求索 · 终端智能体"}
              </div>
            </div>
          </div>
          <p className="text-sm text-ink-soft max-w-md leading-relaxed">
            {isZh
              ? "基于 DeepSeek V4 的开源终端编程智能体。MIT 许可证。由一位维护者从得克萨斯独立维护。欢迎提交 Pull Request。"
              : "Open-source terminal-native coding agent built on DeepSeek V4. MIT licensed. Maintained from a small workshop in Texas. Pull requests welcome."}
          </p>
          <div className="font-mono text-[0.7rem] text-ink-mute uppercase tracking-widest">
            {isZh ? "用心制作 · Made with care" : "Made with care · 用心制作"}
          </div>
          {/* Mirror sources — prominent on zh */}
          {isZh && (
            <div className="pt-2 border-t border-paper-line/20">
              <div className="eyebrow mb-2 text-ink-mute">镜像源 / Mirror</div>
              <div className="flex flex-wrap gap-3 text-xs">
                {GITEE_ENABLED && <a href="https://gitee.com/Hmbown/deepseek-tui" className="text-indigo hover:underline" target="_blank" rel="noopener">Gitee 镜像</a>}
                <a href="https://npmmirror.com/package/deepseek-tui" className="text-indigo hover:underline" target="_blank" rel="noopener">npmmirror</a>
                <a href="https://mirrors.tuna.tsinghua.edu.cn/help/crates.io-index.html" className="text-indigo hover:underline" target="_blank" rel="noopener">Tuna crates.io</a>
              </div>
            </div>
          )}
        </div>

        {cols.map((c) => (
          <div key={c.title}>
            <div className="eyebrow mb-3">
              {isZh ? c.title : `${c.title} · `}
              {!isZh && "cn" in c && <span className="font-cjk normal-case tracking-normal">{(c as { cn?: string }).cn}</span>}
            </div>
            <ul className="space-y-2">
              {c.items.map((it) => (
                <li key={it.href}>
                  <Link href={it.href} className="text-sm text-ink hover:text-indigo transition-colors">
                    {it.label}
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      <div className="hairline-t">
        <div className="mx-auto max-w-[1400px] px-6 py-4 flex flex-col gap-2 text-[0.78rem] text-ink-soft">
          <div>
            {isZh ? "安全报告、负责任披露、漏洞协调 — " : "For security reports, responsible disclosure, or vulnerability coordination — "}
            <a href="mailto:security@deepseek-tui.com" className="font-mono text-ink hover:text-indigo">security@deepseek-tui.com</a>
          </div>
          <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 font-mono text-[0.7rem] text-ink-mute uppercase tracking-widest">
            <span>© {new Date().getFullYear()} · DeepSeek TUI · Hmbown</span>
            <span className="font-cjk normal-case tracking-normal">
              {isZh ? "本网站由 DeepSeek V4-Flash 协助维护" : "本网站由 DeepSeek V4-Flash 协同维护"}
            </span>
          </div>
        </div>
      </div>
    </footer>
  );
}
