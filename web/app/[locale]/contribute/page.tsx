import Link from "next/link";
import { InstallCodeBlock } from "@/components/install-code-block";
import { Seal } from "@/components/seal";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return {
    title: isZh ? "参与贡献 · CodeWhale" : "Contribute · CodeWhale",
    description: isZh
      ? "如何提交议题、发送合并请求、加入 CodeWhale 社区。"
      : "How to file issues, send pull requests, and join the CodeWhale community.",
  };
}

const stepsEn = [
  {
    n: "①",
    title: "Find a thread to pull",
    cn: "选择切入点",
    body: "Browse open issues. The good first issue label means the path is clear. The help wanted label means the path is open but contested. Anything else, ask first.",
    cta: { label: "Open issues", href: "https://github.com/Hmbown/CodeWhale/issues" },
  },
  {
    n: "②",
    title: "Fork and branch",
    cn: "复刻并分支",
    body: "git clone your fork, then git checkout -b feat/short-name or fix/short-name. We use conventional commits — feat:, fix:, docs:, refactor:, test:, chore:.",
    cta: { label: "Repo on GitHub", href: "https://github.com/Hmbown/CodeWhale" },
  },
  {
    n: "③",
    title: "Match the local checks",
    cn: "本地检查",
    body: "CI runs cargo fmt --all -- --check, cargo clippy --workspace --all-targets --all-features --locked -- -D warnings, and cargo test --workspace --all-features --locked. Run them before you push.",
    cta: { label: "Contributing guide", href: "https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md" },
  },
  {
    n: "④",
    title: "Open the PR",
    cn: "提交合并",
    body: "PR description should explain WHY, not WHAT (the diff covers what). Link the issue. The maintainer reviews everything personally — response times vary.",
    cta: { label: "PR template", href: "https://github.com/Hmbown/CodeWhale/blob/main/.github/PULL_REQUEST_TEMPLATE.md" },
  },
];

const stepsZh = [
  {
    n: "①",
    title: "选择切入点",
    cn: "Find a thread",
    body: "浏览 open issues。good first issue 标签意味着路径清晰。help wanted 标签意味着路径开放但有争议。其他情况请先询问。",
    cta: { label: "查看议题", href: "https://github.com/Hmbown/CodeWhale/issues" },
  },
  {
    n: "②",
    title: "复刻并创建分支",
    cn: "Fork & branch",
    body: "git clone 你的复刻，然后 git checkout -b feat/short-name 或 fix/short-name。使用约定式提交——feat:、fix:、docs:、refactor:、test:、chore:。",
    cta: { label: "GitHub 仓库", href: "https://github.com/Hmbown/CodeWhale" },
  },
  {
    n: "③",
    title: "通过本地检查",
    cn: "Local checks",
    body: "CI 运行 cargo fmt --all -- --check、cargo clippy --workspace --all-targets --all-features --locked -- -D warnings 和 cargo test --workspace --all-features --locked。推送前请先运行。",
    cta: { label: "贡献指南", href: "https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md" },
  },
  {
    n: "④",
    title: "提交 PR",
    cn: "Open the PR",
    body: "PR 描述应说明「为什么」而非「做了什么」（diff 已经展示了做了什么）。关联相关 issue。维护者亲自审查所有 PR——响应时间视情况而定。",
    cta: { label: "PR 模板", href: "https://github.com/Hmbown/CodeWhale/blob/main/.github/PULL_REQUEST_TEMPLATE.md" },
  },
];

const recursivePromptEn = `You are running inside CodeWhale on DeepSeek V4 Pro.

Improve CodeWhale itself by finding exactly one small, reviewable friction point in the harness, docs, tests, or contributor workflow.

Prefer bug fixes, regression tests, clearer docs, sharper error messages, or one narrow contributor-experience improvement. Do not change product direction, provider policy, telemetry, sponsorship, branding, auth, sandbox, release/publishing, or global prompts unless the maintainer explicitly asked for that exact scope.

Working rules:
1. Inspect the repo and current open issues before editing.
2. Choose one issue, TODO, failing test, docs ambiguity, confusing error, or repeated papercut.
3. State the exact target and why it is small enough to review.
4. Reproduce the problem when possible.
5. Make the minimum patch.
6. Run the smallest relevant checks first.
7. Stop after one patch.

Output: issue summary, files changed, checks run, risks or follow-up, and a suggested PR title.`;

const recursivePromptZh = `你正在 DeepSeek V4 Pro 驱动的 CodeWhale 中运行。

请改进 CodeWhale 本身：只找一个很小、可审查的摩擦点，范围可以是智能体框架、文档、测试或贡献流程。

优先处理 bug 修复、回归测试、文档澄清、错误信息改进，或一个很窄的贡献者体验问题。除非维护者明确要求，否则不要改产品方向、提供商策略、遥测、赞助、品牌、认证、沙箱、发布流程或全局提示词。

工作规则：
1. 编辑前先阅读仓库和当前 open issues。
2. 只选择一个 issue、TODO、失败测试、文档歧义、错误信息或重复出现的小摩擦点。
3. 先说明目标是什么，以及为什么它足够小、适合审查。
4. 尽可能复现问题。
5. 写最小补丁。
6. 先运行最小相关检查。
7. 一个补丁完成后就停止。

输出：发现的问题摘要、修改文件、已运行检查、风险或后续事项，以及建议的 PR 标题。`;

export default async function ContributePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const steps = isZh ? stepsZh : stepsEn;
  const recursivePrompt = isZh ? recursivePromptZh : recursivePromptEn;

  return (
    <>
      {isZh ? (
        <>
          <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="参" />
              <div className="eyebrow">参与 · Contribute</div>
            </div>
            <h1 className="font-display tracking-crisp">参与贡献</h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
              无 CLA，无赞助商优先通道，维护者只有一人。小而聚焦的 PR 最容易合并——附上真实测试，以及能告诉审查者你在想什么的文字。
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

          <section id="recursive-harness" className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10 min-w-0">
            <div className="lg:col-span-4 min-w-0">
              <Seal char="递" />
              <div className="eyebrow mt-5 mb-3">100:1 模型 · Recursive harness</div>
              <h2 className="font-display text-3xl">让 CodeWhale 改进 CodeWhale</h2>
              <p className="mt-4 text-ink-soft leading-[1.9] tracking-wide">
                100:1 提示词把 DeepSeek V4 Pro 的缓存注意力、工具调用与子智能体，凝结为一个可审查的小补丁——而不是一堆点子。
              </p>
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/RECURSIVE_SELF_IMPROVEMENT.md" className="inline-block mt-5 font-mono text-[0.72rem] uppercase tracking-wider text-indigo hover:underline">
                完整多语言提示词 →
              </Link>
            </div>
            <div className="lg:col-span-8 min-w-0">
              <InstallCodeBlock cmd={recursivePrompt} copyLabel="复制" copiedLabel="已复制" />
            </div>
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
                <Link href="https://github.com/Hmbown/CodeWhale/blob/main/CODE_OF_CONDUCT.md" className="body-link mx-1">行为准则</Link>
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
git clone git@github.com:YOU/CodeWhale
cd CodeWhale
git checkout -b feat/your-thing

# 本地构建运行
cargo build
cargo run --bin codewhale

# 检查（与 CI 完全一致）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked

# 一致性验证
cargo test -p codewhale-tui-core --test snapshot --locked
cargo test -p codewhale-protocol --test parity_protocol --locked
cargo test -p codewhale-state --test parity_state --locked

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
              <div className="eyebrow">Contribute · 参与</div>
            </div>
            <h1 className="font-display tracking-crisp">Contribute</h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-relaxed">
              No CLA. No sponsor lockouts. One maintainer. Small, focused PRs land fastest — please bring real test coverage and prose that tells the reviewer what you were thinking.
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

          <section id="recursive-harness" className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10 min-w-0">
            <div className="lg:col-span-4 min-w-0">
              <Seal char="递" />
              <div className="eyebrow mt-5 mb-3">100-to-1 model · 递归框架</div>
              <h2 className="font-display text-3xl">Let CodeWhale improve CodeWhale</h2>
              <p className="mt-4 text-ink-soft leading-relaxed">
                The 100-to-1 prompt turns DeepSeek V4 Pro's cached attention, tool use, and sub-agents into one small, reviewable patch — not a pile of ideas.
              </p>
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/RECURSIVE_SELF_IMPROVEMENT.md" className="inline-block mt-5 font-mono text-[0.72rem] uppercase tracking-wider text-indigo hover:underline">
                Full multilingual prompt →
              </Link>
            </div>
            <div className="lg:col-span-8 min-w-0">
              <InstallCodeBlock cmd={recursivePrompt} />
            </div>
          </section>

          <section className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10">
            <div className="lg:col-span-5">
              <Seal char="规" />
              <h2 className="font-display text-3xl mt-4">
                House rules <span className="font-cjk text-indigo text-2xl ml-2">规约</span>
              </h2>
              <p className="text-ink-soft mt-4 leading-relaxed">
                Short version: build the thing, don't polish the meta. The full
                <Link href="https://github.com/Hmbown/CodeWhale/blob/main/CODE_OF_CONDUCT.md" className="body-link mx-1">Code of Conduct</Link>
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
git clone git@github.com:YOU/CodeWhale
cd CodeWhale
git checkout -b feat/your-thing

# build and run locally
cargo build
cargo run --bin codewhale

# checks (matches CI exactly)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked

# parity gates
cargo test -p codewhale-tui-core --test snapshot --locked
cargo test -p codewhale-protocol --test parity_protocol --locked
cargo test -p codewhale-state --test parity_state --locked

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
