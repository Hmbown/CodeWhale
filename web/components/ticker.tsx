import type { FeedItem } from "@/lib/types";
import { relativeAge, relativeTime } from "@/lib/github";
import { splitToken } from "@/lib/i18n/dictionaries";
import { truncateChars } from "@/lib/truncate";

/**
 * The wire strip under the masthead: what actually happened in the
 * repository, in the reader's language, with the person who did it named.
 *
 * Everything here is GitHub's own record — merge state, handles, and
 * GitHub's `author_association` first-timer verdict, all from the list
 * payloads `fetchFeed` already pulls. Nothing is summarized, ranked, or
 * generated; an empty feed renders nothing at all rather than a skeleton.
 *
 * Titles and handles are content and stay verbatim. The chrome around them —
 * the event verb, the by-line, the first-contribution mark — comes from the
 * caller's dictionary, and the age comes from CLDR via the locale's own
 * `dateLocale`.
 */
export interface TickerLabels {
  /** Han-seal live label and its mono tag. */
  liveLabel: string;
  liveTag: string;
  /** aria-label for the strip's landmark. */
  ariaLabel: string;
  /** Event verbs. Drafts never reach the strip — see `EVENT_STATES`. */
  merged: string;
  opened: string;
  closed: string;
  released: string;
  /** Mark on a newcomer's contribution, e.g. "first contribution". */
  firstContribution: string;
  /** By-line template carrying a `{handle}` token, e.g. "by {handle}". */
  by: string;
  /** BCP 47 tag driving `Intl.RelativeTimeFormat` (chrome.dateLocale). */
  dateLocale: string;
}

/**
 * States the strip reports. A draft pull request is the one thing here that
 * is not an event — it is work its own author has marked not-ready — and on a
 * repository where agents open them in batches it crowds out the merges and
 * the people behind them. It reappears the moment it is opened or merged.
 */
const EVENT_STATES: readonly FeedItem["state"][] = ["merged", "open", "closed", "published"];

/** The state a feed item is in → the verb the strip prints for it. */
function verbFor(state: FeedItem["state"], labels: TickerLabels): string {
  switch (state) {
    case "merged":
      return labels.merged;
    case "closed":
      return labels.closed;
    case "published":
      return labels.released;
    default:
      return labels.opened;
  }
}

/**
 * Locale age formatter. `Intl.RelativeTimeFormat` is unavailable or missing
 * locale data on some runtimes; falling back to the compact English form is
 * an honest degradation, an empty timestamp is not.
 */
function ageFormatter(dateLocale: string): (iso: string) => string {
  let rtf: Intl.RelativeTimeFormat | undefined;
  try {
    rtf = new Intl.RelativeTimeFormat(dateLocale, { numeric: "auto", style: "narrow" });
  } catch {
    rtf = undefined;
  }
  return (iso: string) => {
    if (!rtf) return relativeTime(iso);
    const { value, unit } = relativeAge(iso);
    return rtf.format(value, unit);
  };
}

function TickerEntry({
  item,
  labels,
  age,
  hidden,
}: {
  item: FeedItem;
  labels: TickerLabels;
  age: string;
  hidden: boolean;
}) {
  const isRelease = item.kind === "release";
  // A release named after its own tag would print the tag twice.
  const title = isRelease && item.title === item.tag ? "" : item.title;
  const [beforeHandle, afterHandle] = splitToken(labels.by, "handle");

  return (
    <span className="ticker-item" aria-hidden={hidden || undefined}>
      <span className="ticker-verb" data-event={item.state}>
        {verbFor(item.state, labels)}
      </span>
      {isRelease ? (
        <span className="ticker-tag tabular">{item.tag}</span>
      ) : (
        <span className="ticker-num tabular">#{item.number}</span>
      )}
      {title ? (
        <span className="ticker-title">{truncateChars(title, 70)}</span>
      ) : null}
      {item.author ? (
        <span className="ticker-by">
          {beforeHandle}
          <span className="ticker-handle">@{item.author}</span>
          {afterHandle}
        </span>
      ) : null}
      {item.firstTimeContributor ? (
        <span className="ticker-first">{labels.firstContribution}</span>
      ) : null}
      <span className="ticker-age tabular">{age}</span>
      <span className="ticker-sep" aria-hidden>
        ◆
      </span>
    </span>
  );
}

export function Ticker({ items, labels }: { items: FeedItem[]; labels: TickerLabels }) {
  // Newest event first — the verb and the age describe the same moment, so
  // the strip reads as a wire and never dates a merge by a later comment.
  const ordered = items
    .filter((item) => EVENT_STATES.includes(item.state))
    .sort((a, b) => +new Date(b.eventAt ?? b.updatedAt) - +new Date(a.eventAt ?? a.updatedAt));

  // Nothing to report is reported as nothing.
  if (!ordered.length) return null;

  const formatAge = ageFormatter(labels.dateLocale);
  // Seamless loop: the track translates -50%, so both halves must be the same
  // flat run of children. The second half is hidden from assistive tech.
  const doubled = [...ordered, ...ordered];

  // Loop duration: never faster than the original 80s baseline; longer feeds
  // (CJK titles run 2-4x wider than English) get proportionally more time so
  // every locale scrolls at a readable pace.
  const glyphCount = ordered.reduce(
    (sum, item) => sum + (item.title?.length ?? 0) + 24,
    0,
  );
  const durationSeconds = Math.max(80, Math.round(glyphCount / 22));

  return (
    <div className="hairline-t hairline-b bg-paper-deep overflow-hidden">
      <div className="site-container flex items-stretch">
        <div className="bg-ink text-paper px-4 py-2 flex items-center shrink-0 gap-2">
          <span className="w-1.5 h-1.5 bg-indigo rounded-full inline-block animate-pulse" />
          <span className="font-cjk text-sm font-semibold tracking-wider">{labels.liveLabel}</span>
          <span className="font-mono text-[0.55rem] uppercase tracking-widest text-paper-deep/60 ml-1 self-end mb-0.5">
            {labels.liveTag}
          </span>
        </div>
        <div className="ticker-viewport" role="group" aria-label={labels.ariaLabel}>
          <div
            className="ticker-track py-2 font-mono text-[0.78rem]"
            style={{ animationDuration: `${durationSeconds}s` }}
          >
            {doubled.map((item, i) => (
              <TickerEntry
                key={`${item.url}-${i}`}
                item={item}
                labels={labels}
                age={formatAge(item.eventAt ?? item.updatedAt)}
                hidden={i >= ordered.length}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
