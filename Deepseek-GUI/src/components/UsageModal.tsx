// 用量统计弹窗：/cost、/tokens 命令展示 Token 与费用

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { UsageAggregation } from "../api/types";
import type { Locale } from "../i18n";

interface UsageModalProps {
  client: RuntimeClient;
  locale: Locale;
  /** 当前活动线程 id（高亮对应行） */
  activeThreadId: string | null;
  onClose: () => void;
}

/** Token 用量与费用模态框 */
export function UsageModal({ client, locale, activeThreadId, onClose }: UsageModalProps) {
  const zh = locale === "zh";
  const [data, setData] = useState<UsageAggregation | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setData(await client.getUsage());
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const t = data?.totals;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="usage-modal" onClick={(e) => e.stopPropagation()}>
        <div className="usage-modal-head">
          <h3>{zh ? "Token 用量" : "Token usage"}</h3>
          <button type="button" className="settings-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>

        {loading && <p className="usage-modal-muted">{zh ? "加载中…" : "Loading…"}</p>}
        {error && <p className="usage-modal-error">{error}</p>}

        {!loading && t && (
          <div className="usage-modal-body">
            <div className="usage-totals-grid">
              <div className="usage-stat">
                <span className="usage-stat-label">{zh ? "输入" : "Input"}</span>
                <span className="usage-stat-value">{t.input_tokens.toLocaleString()}</span>
              </div>
              <div className="usage-stat">
                <span className="usage-stat-label">{zh ? "输出" : "Output"}</span>
                <span className="usage-stat-value">{t.output_tokens.toLocaleString()}</span>
              </div>
              <div className="usage-stat">
                <span className="usage-stat-label">{zh ? "推理" : "Reasoning"}</span>
                <span className="usage-stat-value">{t.reasoning_tokens.toLocaleString()}</span>
              </div>
              <div className="usage-stat">
                <span className="usage-stat-label">{zh ? "费用 (USD)" : "Cost (USD)"}</span>
                <span className="usage-stat-value">${t.cost_usd.toFixed(4)}</span>
              </div>
              <div className="usage-stat">
                <span className="usage-stat-label">{zh ? "回合数" : "Turns"}</span>
                <span className="usage-stat-value">{t.turns}</span>
              </div>
            </div>

            {data.buckets && data.buckets.length > 0 && (
              <>
                <h4 className="usage-section-title">
                  {zh ? `按 ${data.group_by}` : `By ${data.group_by}`}
                </h4>
                <div className="usage-groups">
                  {data.buckets.map((g) => (
                    <div
                      key={g.key}
                      className={`usage-group-row${g.key === activeThreadId ? " active" : ""}`}
                    >
                      <span className="usage-group-key" title={g.key}>
                        {g.key.slice(0, 16)}
                      </span>
                      <span className="usage-group-meta">
                        {g.input_tokens.toLocaleString()} in · {g.output_tokens.toLocaleString()} out ·
                        ${g.cost_usd.toFixed(4)}
                      </span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        )}

        <div className="usage-modal-foot">
          <button type="button" className="btn btn-mini" onClick={() => void load()}>
            {zh ? "刷新" : "Refresh"}
          </button>
        </div>
      </div>
    </div>
  );
}
