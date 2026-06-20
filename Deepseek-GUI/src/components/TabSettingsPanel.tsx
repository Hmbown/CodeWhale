// 设置页：Tab 内联补全（仿 Cursor Tab 面板）

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import {
  loadTabCompletionSettings,
  saveTabCompletionSettings,
  type TabCompletionSettings,
} from "../utils/tabCompletionSettings";

interface TabSettingsPanelProps {
  locale: Locale;
}

/** 单行开关行 */
function ToggleRow({
  title,
  desc,
  checked,
  disabled,
  onChange,
}: {
  title: string;
  desc: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className={`tab-setting-row${disabled ? " is-disabled" : ""}`}>
      <div className="tab-setting-text">
        <div className="tab-setting-title">{title}</div>
        <div className="tab-setting-desc">{desc}</div>
      </div>
      <input
        type="checkbox"
        className="tab-setting-toggle"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

/** Tab 补全设置面板 */
export function TabSettingsPanel({ locale }: TabSettingsPanelProps) {
  const zh = locale === "zh";
  const [settings, setSettings] = useState<TabCompletionSettings>(() => loadTabCompletionSettings());

  /** 更新单项并持久化 */
  const patch = useCallback((partial: Partial<TabCompletionSettings>) => {
    setSettings((prev) => {
      const next = { ...prev, ...partial };
      saveTabCompletionSettings(next);
      return next;
    });
  }, []);

  useEffect(() => {
    setSettings(loadTabCompletionSettings());
  }, []);

  return (
    <div className="settings-section tab-settings">
      <h3 className="settings-section-title">{zh ? "Tab" : "Tab"}</h3>
      <p className="settings-section-desc">
        {zh
          ? "编辑器内 AI 内联补全（类似 Cursor Tab）：根据光标上下文预测下一行或多行代码，Tab 接受。"
          : "AI inline completions in the editor (Cursor Tab style). Tab to accept."}
      </p>

      <ToggleRow
        title={zh ? "Cursor Tab" : "Cursor Tab"}
        desc={
          zh
            ? "基于光标上下文的多行补全（需已配置 API Key）。"
            : "Context-aware multi-line suggestions (requires API Key)."
        }
        checked={settings.enabled}
        onChange={(v) => patch({ enabled: v })}
      />

      <ToggleRow
        title={zh ? "部分接受" : "Partial Accepts"}
        desc={
          zh
            ? "Ctrl+→ 仅接受建议的下一个词。"
            : "Accept the next word via Ctrl+RightArrow."
        }
        checked={settings.partialAccept}
        onChange={(v) => patch({ partialAccept: v })}
      />

      <ToggleRow
        title={zh ? "注释内建议" : "Suggestions While Commenting"}
        desc={zh ? "在注释区域也触发 Tab 补全。" : "Allow Tab in comment regions."}
        checked={settings.suggestInComments}
        onChange={(v) => patch({ suggestInComments: v })}
      />

      <ToggleRow
        title={zh ? "仅空白建议" : "Whitespace-Only Suggestions"}
        desc={
          zh
            ? "允许换行、缩进等纯空白类建议。"
            : "Suggest edits that modify whitespace only."
        }
        checked={settings.whitespaceOnly}
        onChange={(v) => patch({ whitespaceOnly: v })}
      />

      <ToggleRow
        title={zh ? "自动 Import（TypeScript）" : "Imports (TypeScript)"}
        desc={
          zh
            ? "TS/JS 补全时，若用到未导入的符号则自动补上 import 语句。"
            : "Auto-add missing imports for TS/JS completions."
        }
        checked={settings.autoImportTs}
        onChange={(v) => patch({ autoImportTs: v })}
      />

      <ToggleRow
        title={zh ? "自动 Import（Python）Beta" : "Auto Import for Python BETA"}
        desc={
          zh
            ? "Python 补全时自动补上缺失的 import（实验性）。"
            : "Auto-add missing imports for Python completions (beta)."
        }
        checked={settings.autoImportPy}
        onChange={(v) => patch({ autoImportPy: v })}
      />

      <label className="cfg-field tab-setting-globs">
        <span className="cfg-label">{zh ? "忽略文件" : "Ignored Files"}</span>
        <input
          className="cfg-input"
          placeholder="*.md, **/generated/**"
          value={settings.ignoredGlobs}
          onChange={(e) => patch({ ignoredGlobs: e.target.value })}
        />
        <span className="cfg-tip">
          {zh ? "Glob 模式，逗号分隔；这些文件不触发 Tab 补全。" : "Glob patterns, comma-separated."}
        </span>
      </label>
    </div>
  );
}
