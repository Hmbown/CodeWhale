import Link from "next/link";
import { Seal } from "@/components/seal";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "参与贡献 · DeepSeek TUI" : "Contribute · DeepSeek TUI",
    description: isZh
      ? "如何提交议题、发送合并请求、加入 deepseek-tui 社区。"
      : "How to file issues, send pull requests, and join the deepseek-tui community.",
  };
}

const stepsEn = [
  {
    n: "①",
    title: "Find a thread to pull",
    cn: "选择切入点",
    body: "Browse open issues. The good first issue label means the path is clear. The help wanted label means the path is open but contested. Anything else, ask first.",
    cta: { label: "Open issues", href: "https://github.com/Hmbown/deepseek-tui/issues" },
  },
  {
    n: "②",
    title: "Fork and branch",
    cn: "复刻并分支",
    body: "git clone your fork, then git checkout -b feat/short-name or fix/short-name. We use conventional commits — feat:, fix:, docs:, refactor:, test:, chore:.",
    cta: { label: "Repo on GitHub", href: "https://github.com/Hmbown/deepseek-tui" },
  },
  {
    n: "③",
    title: "Match the local checks",
    cn: "本地检查",
    body: "CI runs cargo fmt --all -- --check, cargo clippy --workspace --all-targets --all-features --locked -- -D warnings, and cargo test --workspace --all-features --locked. Run them before you push.",
    cta: { label: "Contributing guide", href: "https://github.com/Hmbown/deepseek-tui/blob/main/CONTRIBUTING.md" },
  },
  {
    n: "④",
    title: "Open the PR",
    cn: "提交合并",
    body: "PR description should explain WHY, not WHAT (the diff covers what). Link the issue. The maintainer reviews everything personally — response times vary.",
    cta: { label: "PR template", href: "https://github.com/Hmbown/deepseek-tui/blob/main/.github/PULL_REQUEST_TEMPLATE.md" },
  },
];

const stepsZh = [
  {
    n: "①",
    title: "选择切入点",
    cn: "Find a thread",
    body: "浏览 open issues。good first issue 标签意味着路径清晰。help wanted 标签意味着路径开放但有争议。其他情况请先询问。",
    cta: { label: "查看议题", href: "https://github.com/Hmbown/deepseek-tui/issues" },
  },
  {
    n: "②",
    title: "复刻并创建分支",
    cn: "Fork & branch",
    body: "git clone 你的复刻，然后 git checkout -b feat/short-name 或 fix/short-name。使用约定式提交——feat:、fix:、docs:、refactor:、test:、chore:。",
    cta: { label: "GitHub 仓库", href: "https://github.com/Hmbown/deepseek-tui" },
  },
  {
    n: "③",
    title: "通过本地检查",
    cn: "Local checks",
    body: "CI 运行 cargo fmt --all -- --check、cargo clippy --workspace --all-targets --all-features --locked -- -D warnings 和 cargo test --workspace --all-features --locked。推送前请先运行。",
    cta: { label: "贡献指南", href: "https://github.com/Hmbown/deepseek-tui/blob/main/CONTRIBUTING.md" },
  },
  {
    n: "④",
    title: "提交 PR",
    cn: "Open the PR",
    body: "PR 描述应说明「为什么」而非「做了什么」（diff 已经展示了做了什么）。关联相关 issue。维护者亲自审查所有 PR——响应时间视情况而定。",
    cta: { label: "PR 模板", href: "https://github.com/Hmbown/deepseek-tui/blob/main/.github/PULL_REQUEST_TEMPLATE.md" },
  },
];

export default async function ContributePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const steps = isZh ? stepsZh : stepsEn;

  return (
    <>
      {isZh ? (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="参" />
              <div className="eyebrow">Section 05 · 参与</div>
            </div>
            <h1 className="font-display tracking-crisp">
              参与贡献 <span className="font-cjk text-indigo text-5xl ml-2">Contribute</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
              无需签署 CLA。没有赞助商优先通道。维护者只有一人；请成为你希望收到的那种贡献者：
              小而聚焦的 PR、真实的测试覆盖、以及能告诉审查者你在想什么的文字。
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-16 hairline-t hairline-b">
            <ol className="grid md:grid-cols-2 lg:grid-cols-4 gap-0 col-rule">
              {steps.map((s) => (
                <li key={s.n} className="p-7">
                  <div className="font-display text-5xl text-indigo mb-3">{s.n}</div>
                  <div className="eyebrow mb-2">{s.cn}</div>
                  <h3 className="font-display text-xl mb-3 leading-tight">{s.title}</h3>
                  <p className="text-sm text-ink-soft leading-[1.9] tracking-wide mb-4">{s.body}</p>
                  <Link href={s.cta.href} className="font-mono text-[0.72rem] uppercase tracking-wider text-indigo hover:underline">
                    {s.cta.label} →
                  </Link>
                </li>
              ))}
            </ol>
          </section>

          {/* 规约 */}
          <section className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10">
            <div className="lg:col-span-5">
              <Seal char="规" />
              <h2 className="font-display text-3xl mt-4">
                规约 <span className="font-cjk text-indigo text-2xl ml-2">House rules</span>
              </h2>
              <p className="text-ink-soft mt-4 leading-[1.9] tracking-wide">
                简而言之：做实事，别折腾元数据。完整的
                <Link href="https://github.com/Hmbown/deepseek-tui/blob/main/CODE_OF_CONDUCT.md" className="body-link mx-1">行为准则</Link>
                是详细版。
              </p>
            </div>
            <div className="lg:col-span-7">
              <ul className="space-y-3">
                {[
                  { k: "欢迎", v: "附带复现步骤的 bug 报告、说明权衡的重构、修复真实歧义的文档 PR。" },
                  { k: "欢迎", v: "能复现 bug 的测试——甚至比修复本身更有价值。" },
                  { k: "欢迎", v: "在 Discussions 中提出有深度的问题。带数据更佳。" },
                  { k: "不欢迎", v: "不理解 diff 的 AI 生成补丁。" },
                  { k: "不欢迎", v: "在代码库或文档中添加托管 SaaS 依赖、遥测或推荐链接。" },
                  { k: "不欢迎", v: "按个人偏好跨仓库重命名。" },
                ].map((r, i) => (
                  <li key={i} className="flex gap-4 hairline-b pb-3">
                    <span className={`font-mono text-[0.72rem] uppercase tracking-widest pt-1 w-10 shrink-0 ${r.k === "欢迎" ? "text-jade" : "text-indigo"}`}>
                      {r.k}
                    </span>
                    <span className="text-sm text-ink-soft leading-[1.9] tracking-wide">{r.v}</span>
                  </li>
                ))}
              </ul>
            </div>
          </section>

          {/* 开发循环 */}
          <section className="bg-paper-deep hairline-t hairline-b">
            <div className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10 min-w-0">
              <div className="lg:col-span-4 min-w-0">
                <div className="eyebrow mb-3">开发循环 · The dev loop</div>
                <h2 className="font-display text-3xl">从克隆到合并</h2>
                <p className="mt-4 text-ink-soft leading-[1.9] tracking-wide">
                  完整流程，可直接复制粘贴。仅限 stable Rust——切勿使用 nightly 特性。
                </p>
              </div>
              <div className="lg:col-span-8 min-w-0">
                <pre className="code-block">
{`# 在 GitHub 上 fork，然后：
git clone git@github.com:YOU/deepseek-tui
cd deepseek-tui
git checkout -b feat/your-thing

# 本地构建运行
cargo build
cargo run --bin deepseek

# 检查（与 CI 完全一致）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked

# 一致性验证
cargo test -p deepseek-tui-core --test snapshot --locked
cargo test -p deepseek-protocol --test parity_protocol --locked
cargo test -p deepseek-state --test parity_state --locked

# 提交 + 推送 + PR
git commit -m "feat: short subject in conventional-commit form"
git push -u origin feat/your-thing
gh pr create --fill`}
                </pre>
              </div>
            </div>
          </section>

        </>
      ) : (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="参" />
              <div className="eyebrow">Section 05 · 参与</div>
            </div>
            <h1 className="font-display tracking-crisp">
              Contribute <span className="font-cjk text-indigo text-5xl ml-2">参与</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-relaxed">
              No CLA. No sponsor lockouts. The maintainer is one person; please be the kind of contributor
              you'd want to receive. Specifically: small focused PRs, real test coverage, and prose that
              tells the reviewer what you were thinking.
            </p>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 pb-16 hairline-t hairline-b">
            <ol className="grid md:grid-cols-2 lg:grid-cols-4 gap-0 col-rule">
              {steps.map((s) => (
                <li key={s.n} className="p-7">
                  <div className="font-display text-5xl text-indigo mb-3">{s.n}</div>
                  <div className="eyebrow mb-2">{s.cn}</div>
                  <h3 className="font-display text-xl mb-3 leading-tight">{s.title}</h3>
                  <p className="text-sm text-ink-soft leading-relaxed mb-4">{s.body}</p>
                  <Link href={s.cta.href} className="font-mono text-[0.72rem] uppercase tracking-wider text-indigo hover:underline">
                    {s.cta.label} →
                  </Link>
                </li>
              ))}
            </ol>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10">
            <div className="lg:col-span-5">
              <Seal char="规" />
              <h2 className="font-display text-3xl mt-4">
                House rules <span className="font-cjk text-indigo text-2xl ml-2">规约</span>
              </h2>
              <p className="text-ink-soft mt-4 leading-relaxed">
                Short version: build the thing, don't polish the meta. The full
                <Link href="https://github.com/Hmbown/deepseek-tui/blob/main/CODE_OF_CONDUCT.md" className="body-link mx-1">Code of Conduct</Link>
                is the long version.
              </p>
            </div>
            <div className="lg:col-span-7">
              <ul className="space-y-3">
                {[
                  { k: "Yes", v: "Bug reports with reproductions, refactors that explain the trade-off, docs PRs that fix a real ambiguity." },
                  { k: "Yes", v: "Tests that demonstrate the bug — even better than fixes." },
                  { k: "Yes", v: "Hard questions in Discussions. Even better if you bring data." },
                  { k: "No", v: "Drive-by AI-generated patches with no understanding of the diff." },
                  { k: "No", v: "Adding hosted SaaS dependencies, telemetry, or referral links to the codebase or docs." },
                  { k: "No", v: "Renaming things across the repo to match your preferences." },
                ].map((r, i) => (
                  <li key={i} className="flex gap-4 hairline-b pb-3">
                    <span className={`font-mono text-[0.72rem] uppercase tracking-widest pt-1 w-10 shrink-0 ${r.k === "Yes" ? "text-jade" : "text-indigo"}`}>
                      {r.k}
                    </span>
                    <span className="text-sm text-ink-soft leading-relaxed">{r.v}</span>
                  </li>
                ))}
              </ul>
            </div>
          </section>

          <section className="bg-paper-deep hairline-t hairline-b">
            <div className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10 min-w-0">
              <div className="lg:col-span-4 min-w-0">
                <div className="eyebrow mb-3">The dev loop · 开发循环</div>
                <h2 className="font-display text-3xl">From clone to merged</h2>
                <p className="mt-4 text-ink-soft leading-relaxed">
                  The full sequence, copy-pasteable. Stable Rust only — never reach for nightly features.
                </p>
              </div>
              <div className="lg:col-span-8 min-w-0">
                <pre className="code-block">
{`# fork on github, then:
git clone git@github.com:YOU/deepseek-tui
cd deepseek-tui
git checkout -b feat/your-thing

# build and run locally
cargo build
cargo run --bin deepseek

# checks (matches CI exactly)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked

# parity gates
cargo test -p deepseek-tui-core --test snapshot --locked
cargo test -p deepseek-protocol --test parity_protocol --locked
cargo test -p deepseek-state --test parity_state --locked

# commit + push + PR
git commit -m "feat: short subject in conventional-commit form"
git push -u origin feat/your-thing
gh pr create --fill`}
                </pre>
              </div>
            </div>
          </section>

        </>
      )}
    </>
  );
}
