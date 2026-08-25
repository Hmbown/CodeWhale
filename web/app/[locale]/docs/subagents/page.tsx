import { Fragment } from "react";
import { getDocsSubagents, splitTokens } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";

/** Role identifiers, in the order the page lists them. Not copy. */
const ROLE_ORDER = [
  "worker",
  "scout",
  "planner",
  "reviewer",
  "builder",
  "verifier",
  "consultant",
  "custom",
] as const;

const CODE_SPANS: Record<string, string> = {
  agentTool: "agent",
  forkContext: "fork_context: true",
  worktreeFlag: "worktree: true",
  branchPattern: "codex/agent-<name>-<id>",
  worktreeDir: ".codewhale-worktrees/",
  writeAuthority: "write_authority",
  writeRoots: "write_roots",
  exactFiles: "exact_files",
  coordinationContracts: "coordination_contracts",
};

function withCodeSpans(template: string) {
  return splitTokens(template).map((part, i) =>
    "token" in part ? (
      <code key={`${i}-${part.token}`} className="inline">
        {CODE_SPANS[part.token] ?? `{${part.token}}`}
      </code>
    ) : (
      <Fragment key={`${i}-text`}>{part.text}</Fragment>
    ),
  );
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsSubagents(locale);
  return buildPageMetadata({
    path: "/docs/subagents",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

export default async function SubagentsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getDocsSubagents(locale);
  const roleDetails = new Map(t.roles);

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h1 className="font-display text-3xl mb-1">{t.overviewTitle}</h1>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewLead}</p>
        <p className={`${t.bodyClassName} mt-3`}>{t.overviewFleetNote}</p>
        <div className="hairline-t mt-6">
          {ROLE_ORDER.map((role) => (
            <section key={role} className="py-4 hairline-b">
              <h3 className="font-display text-lg">{role}</h3>
              <p className={`${t.bodyClassName} mt-1 text-sm`}>{roleDetails.get(role)}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="fork" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.forkTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.forkLead)}</p>
      </section>

      <section id="worktree" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.worktreeTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{withCodeSpans(t.worktreeLead)}</p>
      </section>

      <section id="capacity" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{t.capacityTitle}</h2>
        <p className={`${t.bodyClassName} mt-3`}>{t.capacityLead}</p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">{t.sourceNote}</p>
      </section>
    </section>
  );
}
