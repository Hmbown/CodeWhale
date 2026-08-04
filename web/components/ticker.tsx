import type { FeedItem } from "@/lib/types";
import { relativeTime } from "@/lib/github";

export function Ticker({
  items,
  liveLabel,
  liveTag,
}: {
  items: FeedItem[];
  /** Locale's own live-seal label — never a hardcoded Han string. */
  liveLabel: string;
  liveTag: string;
}) {
  if (!items.length) return null;
  const doubled = [...items, ...items]; // seamless loop
  return (
    <div className="hairline-t hairline-b bg-paper-deep overflow-hidden">
      <div className="mx-auto max-w-[1400px] flex items-stretch">
        <div className="bg-ink text-paper px-4 py-2 flex items-center shrink-0 gap-2">
          <span className="w-1.5 h-1.5 bg-indigo rounded-full inline-block animate-pulse" />
          <span className="font-cjk text-sm font-semibold tracking-wider">{liveLabel}</span>
          <span className="font-mono text-[0.55rem] uppercase tracking-widest text-paper-deep/60 ml-1 self-end mb-0.5">{liveTag}</span>
        </div>
        <div className="flex-1 overflow-hidden relative">
          <div className="ticker-track py-2 font-mono text-[0.78rem]">
            {doubled.map((item, i) => (
              <span key={`${item.url}-${i}`} className="inline-flex items-center gap-2">
                <span className="text-indigo uppercase tracking-wider">{item.kind === "pull" ? "PR" : "ISS"}</span>
                <span className="tabular text-ink-mute">#{item.number}</span>
                <span className="text-ink">{item.title.slice(0, 78)}{item.title.length > 78 ? "…" : ""}</span>
                <span className="text-ink-mute tabular">· {relativeTime(item.updatedAt)}</span>
                <span className="text-paper-line">◆</span>
              </span>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
