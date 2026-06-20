// 项目规则管理（Cursor Rules 兼容）
// 规则存于 {workspace}/.cursor/rules/*.mdc，保存后下一条消息即由后端注入 system prompt

import { useCallback, useEffect, useState } from "react";
import {
  createFile,
  deletePath,
  formatInvokeError,
  isTauri,
  listDir,
  readFile,
  writeFile,
} from "../api/tauri";
import { COMPLIANCE_TYPES } from "../utils/ruleCompliance";
import {
  CURSOR_RULES_REL,
  defaultRuleTemplate,
  parseMdcFile,
  RULE_TEMPLATES,
  rulesDirAbs,
  serializeMdcFile,
  type CursorRule,
} from "../utils/cursorRules";

interface RulesViewProps {
  /** 当前项目根目录 */
  rootPath: string | null;
  onBack: () => void;
  /** 嵌入统一设置页：隐藏顶栏返回 */
  embedded?: boolean;
}

/** 规则编辑表单 */
interface RuleForm {
  id: string;
  path: string;
  isNew: boolean;
  description: string;
  globsText: string;
  alwaysApply: boolean;
  /** compliance 类型，逗号分隔（如 readme_changelog, code_comments） */
  complianceText: string;
  body: string;
}

/**
 * 项目规则面板：列表 + 编辑，格式兼容 Cursor .mdc
 */
export function RulesView({ rootPath, onBack, embedded }: RulesViewProps) {
  const [rules, setRules] = useState<CursorRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<RuleForm | null>(null);
  const [busy, setBusy] = useState(false);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  /** 读取 .cursor/rules 下全部规则文件 */
  const refresh = useCallback(async () => {
    if (!rootPath || !isTauri()) {
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const dir = rulesDirAbs(rootPath);
      let entries: Awaited<ReturnType<typeof listDir>> = [];
      try {
        entries = await listDir(dir);
      } catch {
        entries = [];
      }
      const files = entries.filter(
        (e) => !e.is_dir && /\.(mdc|md)$/i.test(e.name),
      );
      const loaded: CursorRule[] = [];
      for (const f of files.sort((a, b) => a.name.localeCompare(b.name))) {
        const fc = await readFile(f.path);
        if (fc.binary) continue;
        const parsed = parseMdcFile(fc.content);
        loaded.push({
          id: f.name.replace(/\.(mdc|md)$/i, ""),
          path: f.path,
          ...parsed,
        });
      }
      setRules(loaded);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [rootPath]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 从模板新建规则 */
  const onNewFromTemplate = useCallback(
    (templateId: string) => {
      const tpl = RULE_TEMPLATES.find((t) => t.id === templateId);
      if (!tpl || !rootPath) return;
      const slug = tpl.id;
      const sep = rootPath.includes("\\") ? "\\" : "/";
      const path = `${rulesDirAbs(rootPath)}${sep}${slug}.mdc`;
      const body = tpl.build();
      const parsed = parseMdcFile(body);
      setForm({
        id: slug,
        path,
        isNew: true,
        description: parsed.description,
        globsText: parsed.globs.join(", "),
        alwaysApply: parsed.alwaysApply,
        complianceText: parsed.compliance.join(", "),
        body: parsed.body,
      });
      setSavedHint(null);
    },
    [rootPath],
  );

  /** 新建空白规则 */
  const onNew = useCallback(() => {
    const slug = `rule-${Date.now().toString(36)}`;
    if (!rootPath) return;
    const sep = rootPath.includes("\\") ? "\\" : "/";
    const path = `${rulesDirAbs(rootPath)}${sep}${slug}.mdc`;
    const body = defaultRuleTemplate(slug);
    const parsed = parseMdcFile(body);
    setForm({
      id: slug,
      path,
      isNew: true,
      description: parsed.description,
      globsText: parsed.globs.join(", "),
      alwaysApply: parsed.alwaysApply,
      complianceText: parsed.compliance.join(", "),
      body: parsed.body,
    });
    setSavedHint(null);
  }, [rootPath]);

  /** 编辑已有规则 */
  const onEdit = useCallback((r: CursorRule) => {
    setForm({
      id: r.id,
      path: r.path,
      isNew: false,
      description: r.description,
      globsText: r.globs.join(", "),
      alwaysApply: r.alwaysApply,
      complianceText: r.compliance.join(", "),
      body: r.body,
    });
    setSavedHint(null);
  }, []);

  /** 保存规则到磁盘 */
  const onSave = useCallback(async () => {
    if (!form || !rootPath) return;
    setBusy(true);
    try {
      const globs = form.globsText
        .split(/[,;\n]/)
        .map((s) => s.trim())
        .filter(Boolean);
      const compliance = form.complianceText
        .split(/[,;\n]/)
        .map((s) => s.trim())
        .filter(Boolean);
      const content = serializeMdcFile({
        description: form.description,
        globs,
        alwaysApply: form.alwaysApply,
        compliance,
        body: form.body,
      });
      // createFile 会自动创建父目录；勿再 createDir（第二条规则时目录已存在会报错）
      if (form.isNew) {
        await createFile(form.path, content);
      } else {
        await writeFile(form.path, content);
      }
      setForm(null);
      setSavedHint("规则已保存，发送下一条消息时自动生效。");
      await refresh();
    } catch (e) {
      alert(`保存失败：${formatInvokeError(e)}`);
    } finally {
      setBusy(false);
    }
  }, [form, rootPath, refresh]);

  /** 删除规则文件 */
  const onDelete = useCallback(
    async (r: CursorRule) => {
      if (!window.confirm(`确定删除规则「${r.id}」吗？`)) return;
      try {
        await deletePath(r.path);
        if (form?.path === r.path) setForm(null);
        await refresh();
      } catch (e) {
        alert(`删除失败：${formatInvokeError(e)}`);
      }
    },
    [form, refresh],
  );

  if (!isTauri()) {
    return (
      <div className="rules-view">
        <RulesHeader onBack={onBack} onRefresh={refresh} onNew={onNew} disableNew embedded={embedded} />
        <div className="pane-placeholder">项目规则仅在桌面应用中可用。</div>
      </div>
    );
  }

  if (!rootPath) {
    return (
      <div className="rules-view">
        <RulesHeader onBack={onBack} onRefresh={refresh} onNew={onNew} disableNew embedded={embedded} />
        <div className="pane-placeholder">请先在左侧打开项目文件夹。</div>
      </div>
    );
  }

  return (
    <div className="rules-view">
      <RulesHeader
        onBack={onBack}
        onRefresh={() => void refresh()}
        onNew={onNew}
        onNewFromTemplate={onNewFromTemplate}
        embedded={embedded}
      />
      <div className="rules-hint">
        规则文件位于 <code>{CURSOR_RULES_REL}/*.mdc</code>。在 frontmatter 设置 <code>compliance:</code> 可指定回合结束后自动检查的类型（如 readme_changelog、code_comments）。
      </div>
      {savedHint && <div className="banner banner-ok">{savedHint}</div>}
      {error && <div className="banner banner-warn">加载失败：{error}</div>}
      {loading && <div className="pane-placeholder">加载中…</div>}

      {!loading && !form && (
        <div className="rules-list">
          {rules.length === 0 ? (
            <div className="pane-placeholder">
              暂无规则。点击「新建规则」或在此目录手动添加 .mdc 文件。
            </div>
          ) : (
            rules.map((r) => (
              <div key={r.path} className="rules-card">
                <div className="rules-card-main" onClick={() => onEdit(r)}>
                  <div className="rules-card-title">{r.id}</div>
                  <div className="rules-card-desc">{r.description || "（无描述）"}</div>
                  <div className="rules-card-tags">
                    {r.alwaysApply && <span className="rules-tag rules-tag-on">始终应用</span>}
                    {r.compliance.map((c) => (
                      <span key={c} className="rules-tag rules-tag-compliance" title={c}>
                        {complianceLabel(c)}
                      </span>
                    ))}
                    {r.globs.length > 0 && (
                      <span className="rules-tag" title={r.globs.join(", ")}>
                        globs: {r.globs.slice(0, 2).join(", ")}
                        {r.globs.length > 2 ? "…" : ""}
                      </span>
                    )}
                  </div>
                </div>
                <div className="rules-card-actions">
                  <button type="button" className="btn-mini" onClick={() => onEdit(r)}>
                    编辑
                  </button>
                  <button
                    type="button"
                    className="btn-mini btn-danger-text"
                    onClick={() => void onDelete(r)}
                  >
                    删除
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      )}

      {form && (
        <div className="rules-editor">
          <div className="rules-editor-head">
            <span>{form.isNew ? "新建规则" : `编辑 · ${form.id}`}</span>
            <div className="pane-head-btns">
              <button type="button" className="btn-mini" onClick={() => setForm(null)}>
                取消
              </button>
              <button
                type="button"
                className="btn-mini btn-primary-mini"
                disabled={busy}
                onClick={() => void onSave()}
              >
                {busy ? "保存中…" : "保存"}
              </button>
            </div>
          </div>
          <label className="rules-field">
            <span>描述 description</span>
            <input
              className="connbar-input"
              value={form.description}
              onChange={(e) => setForm({ ...form, description: e.target.value })}
              placeholder="简要说明规则用途"
            />
          </label>
          <label className="rules-field rules-check-row">
            <input
              type="checkbox"
              checked={form.alwaysApply}
              onChange={(e) => setForm({ ...form, alwaysApply: e.target.checked })}
            />
            <span>始终应用 alwaysApply（每条消息都注入此规则）</span>
          </label>
          <label className="rules-field">
            <span>合规检查 compliance（逗号分隔，回合结束后自动执行）</span>
            <input
              className="connbar-input"
              value={form.complianceText}
              onChange={(e) => setForm({ ...form, complianceText: e.target.value })}
              placeholder="readme_changelog, code_comments, database_schema, tests_required, api_docs"
            />
          </label>
          <label className="rules-field">
            <span>文件匹配 globs（逗号分隔，可选）</span>
            <input
              className="connbar-input"
              value={form.globsText}
              onChange={(e) => setForm({ ...form, globsText: e.target.value })}
              placeholder="例如 src/**/*.ts, **/*.tsx"
            />
          </label>
          <label className="rules-field rules-field-grow">
            <span>规则正文（Markdown）</span>
            <textarea
              className="rules-textarea"
              value={form.body}
              onChange={(e) => setForm({ ...form, body: e.target.value })}
              rows={12}
              spellCheck={false}
            />
          </label>
        </div>
      )}
    </div>
  );
}

/** 合规类型 id 转短标签 */
function complianceLabel(id: string): string {
  const meta = COMPLIANCE_TYPES.find((t) => t.id === id);
  return meta?.labelZh ?? id;
}

/** 规则面板顶栏 */
function RulesHeader({
  onBack,
  onRefresh,
  onNew,
  onNewFromTemplate,
  disableNew,
  embedded,
}: {
  onBack: () => void;
  onRefresh: () => void;
  onNew: () => void;
  onNewFromTemplate?: (templateId: string) => void;
  disableNew?: boolean;
  embedded?: boolean;
}) {
  return (
    <div className="tasks-head">
      <span className="pane-title">项目规则</span>
      <div className="pane-head-btns rules-template-btns">
        {onNewFromTemplate &&
          RULE_TEMPLATES.map((tpl) => (
            <button
              key={tpl.id}
              type="button"
              className="btn-mini"
              onClick={() => onNewFromTemplate(tpl.id)}
              disabled={disableNew}
              title={`从「${tpl.label}」模板创建`}
            >
              {tpl.label}
            </button>
          ))}
        <button type="button" className="btn-mini" onClick={onNew} disabled={disableNew}>
          ＋ 新建
        </button>
        <button type="button" className="btn-mini" onClick={onRefresh} title="刷新">
          ⟳
        </button>
        {!embedded && (
        <button type="button" className="btn-mini" onClick={onBack} title="返回聊天">
          ← 返回
        </button>
        )}
      </div>
    </div>
  );
}
