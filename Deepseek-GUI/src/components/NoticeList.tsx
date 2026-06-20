// 系统通知条：展示 sandbox.denied / coherence.state 等运行时告警

import type { SystemNotice } from "../state/useConversation";
import { t, type Locale } from "../i18n";

interface NoticeListProps {
  notices: SystemNotice[];
  locale: Locale;
  onDismiss: (id: string) => void;
}

export function NoticeList({ notices, locale, onDismiss }: NoticeListProps) {
  if (notices.length === 0) return null;

  return (
    <div className="notice-list">
      {notices.map((n) => (
        <div key={n.id} className={`notice-bar notice-${n.kind}`}>
          <span className="notice-title">
            {n.kind === "sandbox" ? t("notice.sandbox", locale) : t("notice.coherence", locale)}
            {n.title ? `: ${n.title}` : ""}
          </span>
          {n.detail && <span className="notice-detail">{n.detail}</span>}
          <button type="button" className="notice-dismiss" onClick={() => onDismiss(n.id)} title="关闭">
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
