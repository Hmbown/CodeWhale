// 代码编辑组件：CodeMirror 可编辑 + 语法高亮，支持 Ctrl+S 保存。
// 二进制或超大截断文件仍为只读预览。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { vscodeDark } from "@uiw/codemirror-theme-vscode";
import { EditorView } from "@codemirror/view";
import { readFile, writeFile } from "../api/tauri";
import { langExtensionsForPath } from "../utils/codemirrorLang";
import { useEditorLsp } from "../hooks/useEditorLsp";
import { tabCompletionExtension } from "../codemirror/tabCompletionExtension";
import {
  loadTabCompletionSettings,
  TAB_SETTINGS_CHANGE_EVENT,
} from "../utils/tabCompletionSettings";
import { registerEditorView, unregisterEditorView } from "../utils/editorCommands";

interface CodeViewProps {
  /** 当前打开的文件绝对路径 */
  path: string;
  /** 工作区根目录（LSP rootUri） */
  workspaceRoot?: string | null;
  /** 是否可见（多标签时仅激活项为 true） */
  visible?: boolean;
  /** 嵌入模式：不渲染顶部单文件标签栏（由 EditorPanel 统一管理） */
  embedded?: boolean;
  /** 未保存状态变化回调 */
  onDirtyChange?: (dirty: boolean) => void;
  /** 关闭当前文件 */
  onClose: () => void;
}

/** 代码编辑器固定字号（全局缩放由 useUiZoom 整窗控制） */
const FONT_SIZE = 13;

export function CodeView({
  path,
  workspaceRoot = null,
  visible = true,
  embedded = false,
  onDirtyChange,
  onClose,
}: CodeViewProps) {
  const [text, setText] = useState("");
  /** 磁盘上已保存的内容，用于判断是否有未保存修改 */
  const [savedText, setSavedText] = useState("");
  const [meta, setMeta] = useState<{ truncated: boolean; binary: boolean }>({
    truncated: false,
    binary: false,
  });
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 当前 CodeMirror 视图实例（用于注册到编辑命令总线） */
  const viewRef = useRef<EditorView | null>(null);
  const [editorH, setEditorH] = useState(480);

  const dirty = text !== savedText;
  /** 上报 dirty 状态给父组件（多标签场景） */
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  /** 仅文本文件且未截断时可编辑保存 */
  const editable = !meta.binary && !meta.truncated && !loading && !error;
  const extensions = useMemo(() => langExtensionsForPath(path), [path]);
  /** LSP 补全/悬停/诊断（rust-analyzer、pyright、tsserver 等） */
  const { lspExtensions, lspHint } = useEditorLsp(workspaceRoot, path, editable);
  /** Tab 补全开关（设置页变更时热更新） */
  const [tabEnabled, setTabEnabled] = useState(() => loadTabCompletionSettings().enabled);
  useEffect(() => {
    const sync = () => setTabEnabled(loadTabCompletionSettings().enabled);
    window.addEventListener(TAB_SETTINGS_CHANGE_EVENT, sync);
    return () => window.removeEventListener(TAB_SETTINGS_CHANGE_EVENT, sync);
  }, []);
  /** Cursor Tab 风格 AI 内联补全（与 LSP 互补） */
  const tabExtensions = useMemo(() => {
    if (!editable || !tabEnabled) return [];
    return [tabCompletionExtension(path)];
  }, [path, editable, tabEnabled]);
  /** 固定字号主题（整窗缩放见状态栏 / Ctrl+滚轮） */
  const fontTheme = useMemo(
    () =>
      EditorView.theme({
        "&": { fontSize: `${FONT_SIZE}px` },
        ".cm-gutters": { fontSize: `${FONT_SIZE}px` },
        ".cm-content": {
          fontFamily: '"Cascadia Code", "Consolas", monospace',
        },
      }),
    [],
  );
  const cmExtensions = useMemo(
    () => [...extensions, fontTheme, ...lspExtensions, ...tabExtensions],
    [extensions, fontTheme, lspExtensions, tabExtensions],
  );

  // 切换文件时重新读取
  useEffect(() => {
    if (!path) return;
    let alive = true;
    setLoading(true);
    setError(null);
    setSaveMsg(null);
    readFile(path)
      .then((f) => {
        if (!alive) return;
        setText(f.content);
        setSavedText(f.content);
        setMeta({ truncated: f.truncated, binary: f.binary });
      })
      .catch((err) => alive && setError((err as Error).message))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [path]);

  // 监听中间栏可用高度，让 CodeMirror 填满可视区域
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const sync = () => setEditorH(Math.max(200, el.clientHeight));
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, [path, loading, editable]);

  /** 保存当前编辑内容到磁盘 */
  const save = useCallback(async () => {
    if (!path || !editable || !dirty || saving) return;
    setSaving(true);
    setSaveMsg(null);
    try {
      await writeFile(path, text);
      setSavedText(text);
      setSaveMsg("已保存");
      setTimeout(() => setSaveMsg(null), 2000);
    } catch (e) {
      setSaveMsg(`保存失败：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  }, [path, editable, dirty, saving, text]);

  // Ctrl+S / Cmd+S 快捷键保存
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [save]);

  // 顶部「文件 > 保存」菜单触发：仅当前可见标签响应，避免多标签重复保存
  useEffect(() => {
    if (!visible) return;
    const onSave = () => void save();
    window.addEventListener("ds-editor-save", onSave);
    return () => window.removeEventListener("ds-editor-save", onSave);
  }, [visible, save]);

  // 组件卸载时从编辑命令总线注销视图
  useEffect(() => {
    return () => {
      if (viewRef.current) {
        unregisterEditorView(viewRef.current);
        viewRef.current = null;
      }
    };
  }, []);

  /** 关闭前若有未保存修改则确认 */
  const handleClose = () => {
    if (dirty && !window.confirm("有未保存的修改，确定关闭吗？")) return;
    onClose();
  };

  if (!path) {
    return <div className="code-empty">从左侧选择一个文件查看或编辑。</div>;
  }

  const fileName = path.split(/[\\/]/).pop();

  /** 工具栏：保存 + LSP 状态提示 */
  const toolbar = editable ? (
    <div className="code-tab-actions">
      {lspHint && (
        <span className="code-lsp-hint" title={lspHint}>
          LSP 未就绪
        </span>
      )}
      {saveMsg && <span className="code-save-msg">{saveMsg}</span>}
      <button
        className="btn-mini code-save-btn"
        onClick={() => void save()}
        disabled={!dirty || saving}
        title="保存 (Ctrl+S)"
      >
        {saving ? "保存中…" : "保存"}
      </button>
    </div>
  ) : null;

  return (
    <div className={`code-view${visible ? "" : " code-hidden"}${embedded ? " code-embedded" : ""}`}>
      {!embedded ? (
        <div className="code-tab" title={path}>
          <span className="code-tab-name">
            {dirty && <span className="code-dirty" title="未保存">●</span>}
            {fileName}
            {meta.truncated && <span className="code-flag">（已截断·只读）</span>}
            {meta.binary && <span className="code-flag">（二进制·只读）</span>}
          </span>
          <div className="code-tab-actions-wrap">
            {toolbar}
            <button className="code-tab-close" title="关闭" onClick={handleClose}>
              ×
            </button>
          </div>
        </div>
      ) : (
        toolbar && <div className="code-toolbar">{toolbar}</div>
      )}
      <div className="code-scroll" ref={scrollRef}>
        {loading && <div className="code-loading">读取中…</div>}
        {error && <div className="code-error">{error}</div>}
        {!loading && !error && meta.binary && (
          <div className="code-binary">二进制文件，无法编辑。</div>
        )}
        {!loading && !error && !meta.binary && editable && (
          <div className="code-editor-wrap">
            <CodeMirror
              value={text}
              height={`${editorH}px`}
              theme={vscodeDark}
              extensions={cmExtensions}
              basicSetup={{
                lineNumbers: true,
                foldGutter: true,
                highlightActiveLine: true,
                indentOnInput: true,
                tabSize: 4,
              }}
              onChange={(v) => setText(v)}
              onCreateEditor={(view) => {
                // 注册到编辑命令总线，使顶部菜单的撤销/查找等作用到本编辑器
                viewRef.current = view;
                registerEditorView(view);
              }}
            />
          </div>
        )}
        {!loading && !error && !meta.binary && meta.truncated && (
          <pre className="code-pre code-readonly">{text}</pre>
        )}
      </div>
    </div>
  );
}
