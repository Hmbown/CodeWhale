// 项目规则合规横幅：支持多项缺口（README、注释、数据库、测试、API 等）

import { t, type Locale } from "../i18n";
import type { RuleComplianceNotice } from "../hooks/useRuleCompliance";
import { getChecker } from "../utils/ruleCompliance";

interface RuleComplianceBannerProps {
  notice: RuleComplianceNotice | null;
  locale: Locale;
  onDismiss: () => void;
  onRetry: () => void;
}

/** 展示规则合规缺口列表与一键跟进 */
export function RuleComplianceBanner({ notice, locale, onDismiss, onRetry }: RuleComplianceBannerProps) {
  if (!notice || notice.gaps.length === 0) return null;

  const items = notice.gaps.map((g) => {
    const checker = getChecker(g.gap);
    const detailKey = checker?.detailI18nKey ?? "rule.genericDetail";
    const retryKey = checker?.retryI18nKey ?? "rule.retryGeneric";
    return {
      gap: g.gap,
      ruleIds: g.ruleIds,
      detail: t(detailKey, locale),
      retryLabel: t(retryKey, locale),
    };
  });

  const primaryRetry = items[0]?.retryLabel ?? t("rule.retryGeneric", locale);

  return (
    <div className="banner banner-warn rule-compliance-banner">
      <div className="rule-compliance-text">
        <strong>{t("rule.title", locale)}</strong>
        {items.length === 1 ? (
          <span>{items[0].detail}</span>
        ) : (
          <ul className="rule-compliance-list">
            {items.map((it) => (
              <li key={it.gap}>
                {it.detail}
                {it.ruleIds.length > 0 && (
                  <span className="rule-compliance-ids-inline"> ({it.ruleIds.join(", ")})</span>
                )}
              </li>
            ))}
          </ul>
        )}
        {items.length === 1 && items[0].ruleIds.length > 0 && (
          <span className="rule-compliance-ids">
            {t("rule.fromRules", locale)}: {items[0].ruleIds.join(", ")}
          </span>
        )}
      </div>
      <div className="rule-compliance-actions">
        <button type="button" className="banner-action" onClick={() => void onRetry()}>
          {items.length > 1 ? t("rule.retryAll", locale) : primaryRetry}
        </button>
        <button
          type="button"
          className="rule-compliance-dismiss"
          onClick={onDismiss}
          title={t("rule.dismiss", locale)}
        >
          ×
        </button>
      </div>
    </div>
  );
}
