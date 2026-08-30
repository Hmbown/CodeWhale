import { Fragment, type ReactNode } from "react";
import { splitTokens } from "@/lib/i18n/dictionaries";

/**
 * Typeset a dictionary template whose `{token}` placeholders stand for
 * code-owned literals (commands, env names, config keys, job states). The
 * literals stay in the page; the sentence stays one translated unit and a
 * locale may reorder the tokens freely. An unknown token renders as
 * `{token}` so template/literal drift is visible in review, never silent.
 */
export function withCodeSpans(template: string, spans: Record<string, string>): ReactNode {
  return splitTokens(template).map((part, i) =>
    "token" in part ? (
      <code key={`${i}-${part.token}`} className="inline">
        {spans[part.token] ?? `{${part.token}}`}
      </code>
    ) : (
      <Fragment key={`${i}-text`}>{part.text}</Fragment>
    ),
  );
}

/** A `[term, detail]` reference list, the docs pages' one table shape. */
export function RefRows({
  rows,
  spans = {},
  termSpans = false,
}: {
  rows: readonly (readonly [string, string])[];
  spans?: Record<string, string>;
  /** Typeset the term column as code (command tables). */
  termSpans?: boolean;
}) {
  return (
    <dl className="docs-ref-rows">
      {rows.map(([term, detail]) => (
        <div key={term}>
          <dt>{termSpans ? <code className="inline">{term}</code> : term}</dt>
          <dd>{withCodeSpans(detail, spans)}</dd>
        </div>
      ))}
    </dl>
  );
}
