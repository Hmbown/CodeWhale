/**
 * "See how it decides" — a terminal-styled pane that surfaces REAL reasoning
 * traces from a CodeWhale session, paired with the decision each produced.
 *
 * The point is "show, don't tell": every agent claims to be aligned/trustworthy;
 * CodeWhale can prove it, because the Constitution is observable in the model's
 * reasoning (it cites "Article II", "Article V", etc. as it decides). No other
 * agent can show this because none have a hierarchy the model reasons against.
 *
 * The traces below are faithful excerpts from an actual session — not invented
 * marketing copy. Keep them honest if you edit them (see AGENTS.md Article II).
 */

export type Scene = {
  /** Short tab label for players that show one scene at a time. */
  tab: { en: string; zh: string };
  context: { en: string; zh: string };
  trace: string;
  cites: string[];
  decision: { en: string; zh: string };
};

export const SCENES: Scene[] = [
  {
    tab: { en: "Stale handoff", zh: "过期交接" },
    context: {
      en: "When the prior session's summary didn't match the real repo state…",
      zh: "当上一会话的总结与仓库真实状态不符时……",
    },
    trace:
      "Let me be careful and truthful here. Per Article II (truth) and V (verification), I need to report what's actually in the tree — not what the previous agent claimed.",
    cites: ["Article II · Truth", "Article V · Verification"],
    decision: {
      en: "Reported the uncommitted work the summary left out, instead of repeating the claim.",
      zh: "如实报告了总结遗漏的未提交改动，而不是照搬旧说法。",
    },
  },
  {
    tab: { en: "Conflicting orders", zh: "指令冲突" },
    context: {
      en: "When a “pause” signal conflicted with the user's explicit request to keep going…",
      zh: "当「暂停」信号与用户明确要求继续相冲突时……",
    },
    trace:
      "The user's current request outranks the runtime statute — so I keep executing the task, not stopping on a technicality.",
    cites: ["Article III · User agency", "Article VII · Hierarchy"],
    decision: {
      en: "Finished the work the user actually asked for.",
      zh: "完成了用户真正要求的工作。",
    },
  },
  {
    tab: { en: "Community credit", zh: "社区署名" },
    context: {
      en: "When trimming the README, tempted to drop the contributor wall…",
      zh: "精简 README 时，本可以顺手删掉贡献者名单……",
    },
    trace:
      "Deleting community credit would break the project's own ethos. Relocate it instead — zero names lost.",
    cites: ["Community ethos"],
    decision: {
      en: "Moved 200 lines of credits to docs/CONTRIBUTORS.md and linked from the README.",
      zh: "把 200 行贡献记录迁到 docs/CONTRIBUTORS.md，并在 README 中给出链接。",
    },
  },
];

export function ThinkingTrace({ locale = "en" }: { locale?: string }) {
  const isZh = locale === "zh";
  return (
    <div className="grid gap-6 md:grid-cols-3">
      {SCENES.map((s, i) => (
        <div
          key={i}
          className="hairline-t hairline-b hairline-l hairline-r bg-paper flex flex-col overflow-hidden"
        >
          {/* terminal title bar */}
          <div className="bg-ink text-paper px-4 py-2.5 flex items-center justify-between">
            <div className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-full bg-jade inline-block" />
              <span className="w-2.5 h-2.5 rounded-full bg-ochre inline-block" />
              <span className="w-2.5 h-2.5 rounded-full bg-indigo inline-block" />
              <span className="ml-2.5 font-mono text-[0.66rem] uppercase tracking-widest text-paper-deep">
                codewhale — thinking
              </span>
            </div>
            <span className="font-cjk text-[0.6rem] text-paper-deep/70">
              {isZh ? "推理痕迹" : "reasoning trace"}
            </span>
          </div>

          {/* context */}
          <div className="px-4 pt-4 text-[0.66rem] font-mono uppercase tracking-wider text-ink-mute">
            {isZh ? s.context.zh : s.context.en}
          </div>

          {/* the trace */}
          <pre className="px-4 py-3 font-mono text-[0.82rem] text-ink leading-relaxed whitespace-pre-wrap flex-1">
            <span className="text-indigo">›</span>{" "}
            <span className="text-ink-soft">{s.trace}</span>
          </pre>

          {/* cited authority */}
          <div className="px-4 pb-3 flex flex-wrap gap-1.5">
            {s.cites.map((c) => (
              <span key={c} className="pill text-[0.58rem] tracking-wider">
                {c}
              </span>
            ))}
          </div>

          {/* the decision it produced */}
          <div className="bg-indigo-pale px-4 py-3 hairline-t text-[0.8rem] leading-relaxed text-ink-soft">
            <span className="font-display text-indigo font-semibold mr-1">→</span>
            {isZh ? s.decision.zh : s.decision.en}
          </div>
        </div>
      ))}
    </div>
  );
}