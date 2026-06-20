// 输入区组件（Cursor 风格）：圆角一体化 Composer + 底栏模式/模型 + 圆形发送
// 支持 @ 文件引用、图片粘贴、Enter 发送 / Shift+Enter 换行

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ThreadRecord } from "../api/types";
import { listDir, saveAttachment, isTauri } from "../api/tauri";
import { t, type Locale } from "../i18n";
import { ComposerPillDropdown, type PillDropdownOption } from "./ComposerPillDropdown";
import {
  REASONING_EFFORT_OPTIONS,
  reasoningEffortLabel,
  type ReasoningEffort,
} from "../utils/reasoningEffort";
import {
  filterSlashCommands,
  slashDesc,
  type SlashCommandDef,
} from "../utils/slashCommands";

/** Composer 底栏：模式 / 模型 / 会话安全开关 */
export interface ComposerToolbarProps {
  locale: Locale;
  models: string[];
  modes: string[];
  model: string;
  mode: string;
  onModelChange: (v: string) => void;
  onModeChange: (v: string) => void;
  activeThread: ThreadRecord | null;
  onChangeThreadField: (patch: {
    allow_shell?: boolean;
    trust_mode?: boolean;
    auto_approve?: boolean;
  }) => void;
  showSystemPrompt: boolean;
  onToggleSystemPrompt: () => void;
  /** 推理强度（config.toml） */
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (v: ReasoningEffort) => void;
  /** Shift+Tab 循环推理强度 */
  onCycleReasoningEffort: () => void;
  /** 注册插入 @ 路径（/attach 命令） */
  onRegisterInsert?: (fn: (relPath: string) => void) => void;
}

interface ComposerProps extends ComposerToolbarProps {
  disabled: boolean;
  running: boolean;
  onSend: (text: string) => void;
  onSteer: (text: string) => void;
  onInterrupt: () => void;
  /** 当前项目根目录，用于 @ 文件引用补全 */
  rootPath: string | null;
}

/** @ 引用候选项 */
interface FileCand {
  rel: string;
  name: string;
}

const MAX_FILES = 2000;
/** 输入框自动增高上限（px），达到后仅内部滚动 */
const COMPOSER_INPUT_MAX_H = 160;
const MAX_DEPTH = 6;

/** 模式 pill 前缀图标 */
function modeIcon(mode: string): string {
  if (mode === "agent") return "∞";
  if (mode === "plan") return "◫";
  if (mode === "yolo") return "⚡";
  return "•";
}

/** 模式显示名 */
function modeLabel(mode: string): string {
  if (mode === "agent") return "Agent";
  if (mode === "plan") return "Plan";
  if (mode === "yolo") return "YOLO";
  return mode;
}

/** 模式说明（下拉副标题） */
function modeHint(mode: string, locale: Locale): string {
  if (locale === "zh") {
    if (mode === "plan") return "只读调查，不执行工具";
    if (mode === "agent") return "工具调用，需审批";
    if (mode === "yolo") return "自动批准，全自动";
  } else {
    if (mode === "plan") return "Read-only, no tools";
    if (mode === "agent") return "Tools with approval";
    if (mode === "yolo") return "Auto-approve all";
  }
  return "";
}

/** 模型显示名（缩短过长 id） */
function modelLabel(model: string): string {
  if (model === "deepseek-v4-pro") return "DeepSeek V4 Pro";
  if (model === "deepseek-v4-flash") return "DeepSeek V4 Flash";
  if (model === "auto") return "Auto";
  return model;
}

async function walkFiles(root: string): Promise<FileCand[]> {
  const out: FileCand[] = [];
  let queue: Array<[string, number]> = [[root, 0]];
  while (queue.length > 0 && out.length < MAX_FILES) {
    const next: Array<[string, number]> = [];
    const results = await Promise.all(
      queue.map(([dir]) => listDir(dir).catch(() => [])),
    );
    for (let i = 0; i < results.length; i++) {
      const depth = queue[i][1];
      for (const e of results[i]) {
        if (e.is_dir) {
          if (depth + 1 <= MAX_DEPTH) next.push([e.path, depth + 1]);
        } else {
          const rel = e.path.startsWith(root)
            ? e.path.slice(root.length).replace(/^[\\/]+/, "")
            : e.path;
          out.push({ rel: rel.split("\\").join("/"), name: e.name });
          if (out.length >= MAX_FILES) break;
        }
      }
      if (out.length >= MAX_FILES) break;
    }
    queue = next;
  }
  return out;
}

export function Composer({
  disabled,
  running,
  onSend,
  onSteer,
  onInterrupt,
  rootPath,
  locale,
  models,
  modes,
  model,
  mode,
  onModelChange,
  onModeChange,
  activeThread,
  onChangeThreadField,
  showSystemPrompt,
  onToggleSystemPrompt,
  reasoningEffort,
  onReasoningEffortChange,
  onCycleReasoningEffort,
  onRegisterInsert,
}: ComposerProps) {
  const [text, setText] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);
  const moreRef = useRef<HTMLDivElement>(null);
  const [moreOpen, setMoreOpen] = useState(false);

  const [files, setFiles] = useState<FileCand[]>([]);
  const filesLoadedFor = useRef<string | null>(null);
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionQuery, setMentionQuery] = useState("");
  const [mentionStart, setMentionStart] = useState(0);
  const [sel, setSel] = useState(0);

  /** 斜杠命令补全 */
  const [slashOpen, setSlashOpen] = useState(false);
  const [slashQuery, setSlashQuery] = useState("");
  const [slashSel, setSlashSel] = useState(0);

  /** 供 /attach 插入 @路径 */
  useEffect(() => {
    if (!onRegisterInsert) return;
    onRegisterInsert((relPath: string) => {
      const ins = `@${relPath} `;
      setText((prev) => (prev.trim() ? `${prev.trimEnd()} ${ins}` : ins));
      requestAnimationFrame(() => taRef.current?.focus());
    });
  }, [onRegisterInsert]);

  // 点击外部关闭「更多」菜单
  useEffect(() => {
    if (!moreOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (moreRef.current && !moreRef.current.contains(e.target as Node)) {
        setMoreOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [moreOpen]);

  useEffect(() => {
    filesLoadedFor.current = null;
    setFiles([]);
  }, [rootPath]);

  /** 输入框随内容自动增高，达到上限后不再增高、改为内部滚动 */
  const resizeTextarea = useCallback(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    const next = Math.min(ta.scrollHeight, COMPOSER_INPUT_MAX_H);
    ta.style.height = `${next}px`;
    ta.style.overflowY = ta.scrollHeight > COMPOSER_INPUT_MAX_H ? "auto" : "hidden";
  }, []);

  useEffect(() => {
    resizeTextarea();
  }, [text, resizeTextarea]);

  const ensureFiles = useCallback(async () => {
    if (!rootPath || filesLoadedFor.current === rootPath) return;
    filesLoadedFor.current = rootPath;
    try {
      setFiles(await walkFiles(rootPath));
    } catch {
      setFiles([]);
    }
  }, [rootPath]);

  const candidates = useMemo(() => {
    if (!mentionOpen) return [];
    const q = mentionQuery.toLowerCase();
    const matched = q
      ? files.filter((f) => f.rel.toLowerCase().includes(q))
      : files;
    matched.sort((a, b) => {
      const an = a.name.toLowerCase().includes(q) ? 0 : 1;
      const bn = b.name.toLowerCase().includes(q) ? 0 : 1;
      return an - bn || a.rel.length - b.rel.length;
    });
    return matched.slice(0, 50);
  }, [mentionOpen, mentionQuery, files]);

  /** 斜杠命令补全候选 */
  const slashCandidates = useMemo(() => {
    if (!slashOpen) return [];
    return filterSlashCommands(slashQuery);
  }, [slashOpen, slashQuery]);

  const acceptSlash = useCallback(
    (def: SlashCommandDef) => {
      setText(`/${def.name} `);
      setSlashOpen(false);
      requestAnimationFrame(() => {
        const ta = taRef.current;
        ta?.focus();
        const caret = def.name.length + 2;
        ta?.setSelectionRange(caret, caret);
      });
    },
    [],
  );

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const v = e.target.value;
    setText(v);
    resizeTextarea();
    const pos = e.target.selectionStart ?? v.length;
    const before = v.slice(0, pos);

    // 斜杠命令补全：行首 / 且尚未输入参数
    const slashMatch = v.match(/^\/(\S*)$/);
    if (slashMatch) {
      setSlashOpen(true);
      setSlashQuery(slashMatch[1] ?? "");
      setSlashSel(0);
      setMentionOpen(false);
      return;
    }
    setSlashOpen(false);

    const at = before.lastIndexOf("@");
    if (at >= 0) {
      const token = before.slice(at + 1);
      const prevCh = at > 0 ? before[at - 1] : " ";
      if (!/\s/.test(token) && /\s|^$/.test(prevCh) && rootPath) {
        setMentionStart(at);
        setMentionQuery(token);
        setMentionOpen(true);
        setSel(0);
        void ensureFiles();
        return;
      }
    }
    setMentionOpen(false);
  };

  const acceptMention = useCallback(
    (cand: FileCand) => {
      const ta = taRef.current;
      const pos = ta?.selectionStart ?? text.length;
      const newText =
        text.slice(0, mentionStart) + "@" + cand.rel + " " + text.slice(pos);
      setText(newText);
      setMentionOpen(false);
      requestAnimationFrame(() => {
        const caret = mentionStart + cand.rel.length + 2;
        if (ta) {
          ta.focus();
          ta.setSelectionRange(caret, caret);
        }
      });
    },
    [text, mentionStart],
  );

  const onPaste = useCallback(
    async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      for (let i = 0; i < items.length; i++) {
        const it = items[i];
        if (it.kind === "file" && it.type.startsWith("image/")) {
          e.preventDefault();
          const file = it.getAsFile();
          if (!file) return;
          if (!rootPath) {
            alert(locale === "zh" ? "请先打开项目文件夹再粘贴图片" : "Open a folder first");
            return;
          }
          try {
            const buf = new Uint8Array(await file.arrayBuffer());
            const ext = (it.type.split("/")[1] || "png").replace("+xml", "");
            const name = `paste-${Date.now()}.${ext}`;
            await saveAttachment(rootPath, name, Array.from(buf));
            const rel = `.deepseek/attachments/${name}`;
            const ta = taRef.current;
            const pos = ta?.selectionStart ?? text.length;
            const ins = `@${rel} `;
            setText(text.slice(0, pos) + ins + text.slice(pos));
            requestAnimationFrame(() => {
              const caret = pos + ins.length;
              ta?.focus();
              ta?.setSelectionRange(caret, caret);
            });
          } catch (err) {
            alert(`${locale === "zh" ? "保存图片失败" : "Save failed"}：${(err as Error).message}`);
          }
          return;
        }
      }
    },
    [rootPath, text, locale],
  );

  const submit = () => {
    const trimmed = text.trim();
    if (!trimmed) return;
    if (running) onSteer(trimmed);
    else onSend(trimmed);
    setText("");
    setMentionOpen(false);
    requestAnimationFrame(resizeTextarea);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (slashOpen && slashCandidates.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSlashSel((s) => (s + 1) % slashCandidates.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSlashSel((s) => (s - 1 + slashCandidates.length) % slashCandidates.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        acceptSlash(slashCandidates[slashSel]!);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setSlashOpen(false);
        return;
      }
    }
    if (mentionOpen && candidates.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSel((s) => (s + 1) % candidates.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSel((s) => (s - 1 + candidates.length) % candidates.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        acceptMention(candidates[sel]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMentionOpen(false);
        return;
      }
    }
    // Shift+Tab：循环推理强度（仿 TUI）
    if (e.key === "Tab" && e.shiftKey && !slashOpen && !mentionOpen) {
      e.preventDefault();
      onCycleReasoningEffort();
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    }
  };

  const canSend = !disabled && Boolean(text.trim());
  const allModels = useMemo(() => {
    if (activeThread && !models.includes(activeThread.model)) {
      return [...models, activeThread.model];
    }
    return models;
  }, [models, activeThread]);
  const allModes = useMemo(() => {
    if (activeThread && !modes.includes(activeThread.mode)) {
      return [...modes, activeThread.mode];
    }
    return modes;
  }, [modes, activeThread]);

  const modeOptions: PillDropdownOption[] = useMemo(
    () =>
      allModes.map((m) => ({
        value: m,
        label: modeLabel(m),
        hint: modeHint(m, locale),
      })),
    [allModes, locale],
  );

  const modelOptions: PillDropdownOption[] = useMemo(
    () => allModels.map((m) => ({ value: m, label: modelLabel(m) })),
    [allModels],
  );

  const effortOptions: PillDropdownOption[] = useMemo(
    () =>
      REASONING_EFFORT_OPTIONS.map((e) => ({
        value: e,
        label: reasoningEffortLabel(e, locale),
      })),
    [locale],
  );

  return (
    <div className="composer">
      {slashOpen && slashCandidates.length > 0 && (
        <div className="slash-pop mention-pop">
          {slashCandidates.map((c, i) => (
            <div
              key={c.name}
              className={i === slashSel ? "mention-item active" : "mention-item"}
              onMouseDown={(e) => {
                e.preventDefault();
                acceptSlash(c);
              }}
              onMouseEnter={() => setSlashSel(i)}
            >
              <span className="mention-name">/{c.name}</span>
              <span className="mention-path">{slashDesc(c, locale)}</span>
            </div>
          ))}
        </div>
      )}
      {mentionOpen && candidates.length > 0 && (
        <div className="mention-pop">
          {candidates.map((c, i) => (
            <div
              key={c.rel}
              className={i === sel ? "mention-item active" : "mention-item"}
              onMouseDown={(e) => {
                e.preventDefault();
                acceptMention(c);
              }}
              onMouseEnter={() => setSel(i)}
              title={c.rel}
            >
              <span className="mention-name">{c.name}</span>
              <span className="mention-path">{c.rel}</span>
            </div>
          ))}
        </div>
      )}

      <div className="composer-box">
        <textarea
          ref={taRef}
          className="composer-input"
          placeholder={
            disabled
              ? locale === "zh"
                ? "请先新建或选择一个会话…"
                : "Create or select a chat…"
              : locale === "zh"
                ? "输入消息，@ 引用文件，Enter 发送"
                : "Ask anything, @ for files, Enter to send"
          }
          value={text}
          disabled={disabled}
          rows={1}
          onChange={handleChange}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
        />

        <div className="composer-toolbar">
          <div className="composer-toolbar-left">
            <ComposerPillDropdown
              icon={modeIcon(mode)}
              value={mode}
              options={modeOptions}
              disabled={disabled}
              onChange={onModeChange}
              title={locale === "zh" ? "模式" : "Mode"}
              maxWidth={120}
              menuMinWidth={220}
            />

            <ComposerPillDropdown
              value={model}
              options={modelOptions}
              disabled={disabled}
              onChange={onModelChange}
              title={locale === "zh" ? "模型" : "Model"}
              maxWidth={180}
              menuMinWidth={240}
            />

            <ComposerPillDropdown
              value={reasoningEffort}
              options={effortOptions}
              disabled={disabled || !isTauri()}
              onChange={(v) => onReasoningEffortChange(v as ReasoningEffort)}
              title={locale === "zh" ? "推理强度 (Shift+Tab)" : "Reasoning (Shift+Tab)"}
              maxWidth={120}
              menuMinWidth={160}
            />

            {activeThread && (
              <div className="composer-more" ref={moreRef}>
                <button
                  type="button"
                  className={`composer-icon-btn${moreOpen ? " active" : ""}`}
                  title={locale === "zh" ? "更多设置" : "More settings"}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setMoreOpen((v) => !v);
                  }}
                >
                  ⋯
                </button>
                {moreOpen && (
                  <div className="composer-more-menu">
                    <label className="composer-menu-check">
                      <input
                        type="checkbox"
                        checked={Boolean(activeThread.allow_shell)}
                        onChange={(e) => onChangeThreadField({ allow_shell: e.target.checked })}
                      />
                      {t("thread.shell", locale)}
                    </label>
                    <label className="composer-menu-check">
                      <input
                        type="checkbox"
                        checked={Boolean(activeThread.trust_mode)}
                        onChange={(e) => onChangeThreadField({ trust_mode: e.target.checked })}
                      />
                      {t("thread.trust", locale)}
                    </label>
                    <label className="composer-menu-check">
                      <input
                        type="checkbox"
                        checked={Boolean(activeThread.auto_approve)}
                        onChange={(e) => onChangeThreadField({ auto_approve: e.target.checked })}
                      />
                      {t("thread.autoApprove", locale)}
                    </label>
                    <button
                      type="button"
                      className="composer-menu-item"
                      onClick={() => {
                        onToggleSystemPrompt();
                        setMoreOpen(false);
                      }}
                    >
                      {t("thread.systemPrompt", locale)}
                      {showSystemPrompt ? " ✓" : ""}
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>

          <div className="composer-toolbar-right">
            {running && (
              <button
                type="button"
                className="composer-icon-btn composer-stop"
                title={locale === "zh" ? "打断" : "Stop"}
                onClick={onInterrupt}
              >
                ■
              </button>
            )}
            <button
              type="button"
              className="composer-send"
              title={running ? (locale === "zh" ? "转向" : "Steer") : locale === "zh" ? "发送" : "Send"}
              disabled={!canSend}
              onMouseDown={(e) => {
                if (!canSend) return;
                e.preventDefault();
                submit();
              }}
            >
              ↑
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
