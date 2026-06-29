import Link from "next/link";
import { GITEE_ENABLED, type Locale } from "@/lib/i18n/config";
import { Seal } from "./seal";

const EN_COLS = [
  {
    title: "Product",
    cn: "产品",
    items: [
      { label: "Install", href: "/en/install" },
      { label: "Documentation", href: "/en/docs" },
      { label: "Roadmap", href: "/en/roadmap" },
      { label: "FAQ", href: "/en/faq" },
      { label: "Releases", href: "https://github.com/Hmbown/CodeWhale/releases" },
    ],
  },
  {
    title: "Community",
    cn: "社区",
    items: [
      { label: "Issues", href: "https://github.com/Hmbown/CodeWhale/issues" },
      { label: "Pull Requests", href: "https://github.com/Hmbown/CodeWhale/pulls" },
      { label: "Discussions", href: "https://github.com/Hmbown/CodeWhale/discussions" },
      { label: "Contribute", href: "/en/contribute" },
      { label: "Sponsor CodeWhale", href: "https://github.com/sponsors/Hmbown" },
    ],
  },
  {
    title: "Resources",
    cn: "资源",
    items: [
      { label: "Activity Feed", href: "/en/feed" },
      { label: "npm package", href: "https://www.npmjs.com/package/codewhale" },
      { label: "crates.io (codewhale-cli)", href: "https://crates.io/crates/codewhale-cli" },
      { label: "Code of Conduct", href: "https://github.com/Hmbown/CodeWhale/blob/main/CODE_OF_CONDUCT.md" },
      { label: "Security", href: "https://github.com/Hmbown/CodeWhale/blob/main/SECURITY.md" },
      { label: "License (MIT)", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" },
      { label: "Mirrors (CNB · Tsinghua)", href: "/en/install#other-ways" },
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
      { label: "常见问题", href: "/zh/faq" },
      { label: "版本发布", href: "https://github.com/Hmbown/CodeWhale/releases" },
    ],
  },
  {
    title: "社区",
    items: [
      { label: "议题", href: "https://github.com/Hmbown/CodeWhale/issues" },
      { label: "合并请求", href: "https://github.com/Hmbown/CodeWhale/pulls" },
      { label: "讨论区", href: "https://github.com/Hmbown/CodeWhale/discussions" },
      { label: "参与贡献", href: "/zh/contribute" },
      { label: "支持 CodeWhale", href: "https://github.com/sponsors/Hmbown" },
    ],
  },
  {
    title: "资源",
    items: [
      { label: "活动动态", href: "/zh/feed" },
      { label: "npm 包", href: "https://www.npmjs.com/package/codewhale" },
      { label: "crates.io（codewhale-cli）", href: "https://crates.io/crates/codewhale-cli" },
      { label: "行为准则", href: "https://github.com/Hmbown/CodeWhale/blob/main/CODE_OF_CONDUCT.md" },
      { label: "安全策略", href: "https://github.com/Hmbown/CodeWhale/blob/main/SECURITY.md" },
      { label: "MIT 许可证", href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE" },
      { label: "国内镜像（CNB · 清华 Tuna）", href: "/zh/install#other-ways" },
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
              <div className="font-display text-xl font-semibold">CodeWhale</div>
              <div className="font-cjk text-[0.7rem] text-ink-mute tracking-widest">
                {isZh ? "任何模型 · 开源模型优先" : "any model, open models first"}
              </div>
            </div>
          </div>
          <p className="text-sm text-ink-soft max-w-md leading-relaxed">
            {isZh
              ? "面向开源模型的终端编程智能体。DeepSeek V4 为一级模型。MIT 许可证。由一位维护者从得克萨斯独立维护。欢迎提交 Pull Request。"
              : "Open-model terminal-native coding agent. DeepSeek V4 is first-class. MIT licensed. Maintained from a small workshop in Texas. Pull requests welcome."}
          </p>
          <div className="font-mono text-[0.7rem] text-ink-mute uppercase tracking-widest">
            {isZh ? "用心制作 · Made with care" : "Made with care · 用心制作"}
          </div>
          {/* Provenance / source-of-truth — shown on both locales */}
          <div className="pt-2 border-t border-paper-line/20">
            <div className="eyebrow mb-2 text-ink-mute">
              {isZh ? "来源证明 · Provenance" : "Provenance · 来源证明"}
            </div>
            <p className="text-xs text-ink-soft leading-relaxed mb-2">
              {isZh
                ? "GitHub (github.com/Hmbown/CodeWhale) 为唯一官方源码与发布源。下方镜像仅为中国大陆网络加速，内容经自动同步校验，SHA256 清单一致。"
                : "GitHub (github.com/Hmbown/CodeWhale) is the sole canonical source for code and releases. Mirrors below are China-network accelerators only — content is auto-synced and verified via SHA256 manifests."}
            </p>
            <div className="flex flex-wrap gap-3 text-xs">
              <a href="https://github.com/Hmbown/CodeWhale" className="text-indigo hover:underline font-semibold" target="_blank" rel="noopener">
                {isZh ? "★ 官方 GitHub" : "★ Official GitHub"}
              </a>
              {GITEE_ENABLED && <a href="https://gitee.com/Hmbown/CodeWhale" className="text-ink-soft hover:underline" target="_blank" rel="noopener">{isZh ? "Gitee 镜像" : "Gitee mirror"}</a>}
              <a href="https://cnb.cool/codewhale.net/codewhale" className="text-ink-soft hover:underline" target="_blank" rel="noopener">{isZh ? "CNB 镜像" : "CNB mirror"}</a>
              <a href="https://npmmirror.com/package/codewhale" className="text-ink-soft hover:underline" target="_blank" rel="noopener">npmmirror</a>
              <a href="https://mirrors.tuna.tsinghua.edu.cn/help/crates.io-index.html" className="text-ink-soft hover:underline" target="_blank" rel="noopener">Tuna crates.io</a>
            </div>
          </div>
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
            <a href="mailto:hmbown@gmail.com" className="font-mono text-ink hover:text-indigo">hmbown@gmail.com</a>
          </div>
          <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 font-mono text-[0.7rem] text-ink-mute uppercase tracking-widest">
            <span>© {new Date().getFullYear()} · CodeWhale · Hmbown</span>
            <span>codewhale.net — {isZh ? "唯一官方站点" : "the only official site"}</span>
            <span className="font-cjk normal-case tracking-normal">
              {isZh ? "本网站由 DeepSeek V4-Flash 协助维护" : "Maintained with DeepSeek V4-Flash"}
            </span>
          </div>
        </div>
      </div>
    </footer>
  );
}
