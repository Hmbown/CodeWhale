// 回合结束后按项目规则检测合规缺口，并自动/手动发起跟进

import { useCallback, useEffect, useRef, useState } from "react";
import type { Locale } from "../i18n";
import {
  buildCompliancePrompt,
  detectAllComplianceGaps,
  loadCursorRulesFromWorkspace,
  type RuleComplianceGap,
  type RuleComplianceResult,
} from "../utils/ruleCompliance";

export type { RuleComplianceGap, RuleComplianceResult };

/** 待展示的合规通知（可含多项缺口） */
export interface RuleComplianceNotice {
  gaps: RuleComplianceResult[];
}

interface UseRuleComplianceOptions {
  rootPath: string | null;
  running: boolean;
  lastTurnChangedPaths: string[];
  onFollowUp: (text: string) => Promise<void>;
  locale: Locale;
}

/** 将缺口列表编码为稳定指纹（用于去重） */
function gapsFingerprint(paths: string[], gaps: RuleComplianceResult[]): string {
  const gapPart = gaps
    .map((g) => `${g.gap}:${g.ruleIds.sort().join("+")}`)
    .sort()
    .join("|");
  return `${[...paths].sort().join("|")}::${gapPart}`;
}

/**
 * 监听回合完成，检测 alwaysApply + compliance 规则是否已执行；
 * 首次缺口自动发送合并跟进；仍缺口则展示横幅。
 */
export function useRuleCompliance({
  rootPath,
  running,
  lastTurnChangedPaths,
  onFollowUp,
  locale,
}: UseRuleComplianceOptions) {
  const [notice, setNotice] = useState<RuleComplianceNotice | null>(null);
  const handledKeysRef = useRef<Set<string>>(new Set());
  const autoSentKeysRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (running || !rootPath || lastTurnChangedPaths.length === 0) return;

    let cancelled = false;
    void (async () => {
      const rules = await loadCursorRulesFromWorkspace(rootPath);
      if (cancelled) return;

      const gaps = detectAllComplianceGaps(rules, lastTurnChangedPaths);
      if (gaps.length === 0) {
        setNotice(null);
        return;
      }

      const key = gapsFingerprint(lastTurnChangedPaths, gaps);
      if (handledKeysRef.current.has(key)) return;
      handledKeysRef.current.add(key);

      if (!autoSentKeysRef.current.has(key)) {
        autoSentKeysRef.current.add(key);
        setNotice(null);
        try {
          await onFollowUp(buildCompliancePrompt(gaps, locale));
        } catch {
          setNotice({ gaps });
        }
        return;
      }

      setNotice({ gaps });
    })();

    return () => {
      cancelled = true;
    };
  }, [running, rootPath, lastTurnChangedPaths, onFollowUp, locale]);

  const dismiss = useCallback(() => setNotice(null), []);

  const retryFollowUp = useCallback(async () => {
    if (!notice || notice.gaps.length === 0) return;
    setNotice(null);
    await onFollowUp(buildCompliancePrompt(notice.gaps, locale));
  }, [notice, onFollowUp, locale]);

  return { notice, dismiss, retryFollowUp };
}
