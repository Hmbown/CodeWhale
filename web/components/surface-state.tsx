import type { ReactNode } from "react";
import { getStates } from "@/lib/i18n/dictionaries";

/**
 * Shared surface states — the one empty / loading / error vocabulary every
 * data-bearing page renders. Server-safe (no hooks); a caller that needs a
 * live retry passes a client `RetryAction` through `action`.
 *
 * Rules:
 *  - An empty state is a statement that nothing exists, never a placeholder
 *    pretending something does. Copy comes from the states dictionary.
 *  - A loading state is announced (`role="status"`) and drawn as neutral
 *    skeleton lines whose shimmer is gated on `prefers-reduced-motion`.
 *  - An error state says what failed and offers exactly one recovery.
 */

type Tone = "empty" | "loading" | "error";
type TitleTag = "p" | "h1" | "h2";

function Block({
  tone,
  title,
  body,
  action,
  compact,
  role,
  live,
  titleAs: Title = "p",
}: {
  tone: Tone;
  title: string;
  body?: string;
  action?: ReactNode;
  compact?: boolean;
  role?: "status" | "alert";
  live?: "polite" | "assertive";
  /** A route-level plate that is the whole page owns its `<h1>`. */
  titleAs?: TitleTag;
}) {
  return (
    <div
      className={`state-block state-block-${tone}${compact ? " state-block-compact" : ""}`}
      role={role}
      aria-live={live}
      data-state={tone}
    >
      <span className="state-mark" aria-hidden="true" />
      <div className="state-copy">
        <Title className="state-title">{title}</Title>
        {body && <p className="state-body">{body}</p>}
        {action && <div className="state-actions">{action}</div>}
      </div>
    </div>
  );
}

export function EmptyState({
  locale,
  title,
  body,
  action,
  compact,
  titleAs,
}: {
  locale: string;
  title?: string;
  body?: string;
  action?: ReactNode;
  compact?: boolean;
  titleAs?: TitleTag;
}) {
  const t = getStates(locale);
  return (
    <Block
      tone="empty"
      title={title ?? t.emptyTitle}
      body={body ?? t.emptyBody}
      action={action}
      compact={compact}
      titleAs={titleAs}
    />
  );
}

/**
 * The source was not asked or did not answer. Same plate as an error, same
 * single retry, but the copy says "not loaded" rather than "broken": on an
 * ISR page the next refresh is the honest fix, and nothing is shown in place
 * of the missing record.
 */
export function UnavailableState({
  locale,
  action,
  compact,
}: {
  locale: string;
  action?: ReactNode;
  compact?: boolean;
}) {
  const t = getStates(locale);
  return (
    <Block
      tone="error"
      title={t.unavailableTitle}
      body={t.unavailableBody}
      action={action}
      compact={compact}
      role="status"
    />
  );
}

export function LoadingState({
  locale,
  label,
  lines = 3,
  compact,
}: {
  locale: string;
  label?: string;
  lines?: number;
  compact?: boolean;
}) {
  const t = getStates(locale);
  return (
    <div
      className={`state-block state-block-loading${compact ? " state-block-compact" : ""}`}
      role="status"
      aria-live="polite"
      aria-busy="true"
      data-state="loading"
    >
      <span className="state-mark" aria-hidden="true" />
      <div className="state-copy">
        <p className="state-title">{label ?? t.loadingLabel}</p>
        <div className="state-skeleton" aria-hidden="true">
          {Array.from({ length: lines }, (_, i) => (
            <span key={i} style={{ width: `${88 - i * 14}%` }} />
          ))}
        </div>
      </div>
    </div>
  );
}

export function ErrorState({
  locale,
  title,
  body,
  action,
  compact,
  titleAs,
}: {
  locale: string;
  title?: string;
  body?: string;
  action?: ReactNode;
  compact?: boolean;
  titleAs?: TitleTag;
}) {
  const t = getStates(locale);
  return (
    <Block
      tone="error"
      title={title ?? t.errorTitle}
      body={body ?? t.errorBody}
      action={action}
      compact={compact}
      role="alert"
      titleAs={titleAs}
    />
  );
}
