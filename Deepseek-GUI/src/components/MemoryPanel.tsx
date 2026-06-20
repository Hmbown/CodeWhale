// 记忆 / 笔记 / 锚点面板（对齐 TUI /memory /note /anchor）
// - 记忆 memory：全局 ~/.deepseek/memory.md，纯文本编辑后整文件保存。
// - 笔记 note：工作区 .deepseek/notes.md，条目以 "\n---\n" 分隔，主动查阅用。
// - 锚点 anchor：工作区 .deepseek/anchors.md，压缩后自动重注入，保留关键事实。

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import {
  getAnchors,
  getMemory,
  getNotes,
  isTauri,
  saveAnchors,
  saveMemory,
  saveNotes,
} from "../api/tauri";

interface MemoryPanelProps {
  locale: Locale;
  /** 当前工作区路径（笔记 / 锚点按工作区存储） */
  workspace: string | null;
}

/**
 * 记忆 / 笔记 / 锚点统一管理面板。
 * @param locale 当前语言
 * @param workspace 当前工作区根目录
 */
export function MemoryPanel({ locale, workspace }: MemoryPanelProps) {
  const zh = locale === "zh";

  // 全局记忆
  const [memoryPath, setMemoryPath] = useState("");
  const [memory, setMemory] = useState("");
  // 工作区笔记
  const [notesPath, setNotesPath] = useState("");
  const [notes, setNotes] = useState<string[]>([]);
  const [newNote, setNewNote] = useState("");
  // 工作区锚点
  const [anchorsPath, setAnchorsPath] = useState("");
  const [anchors, setAnchors] = useState<string[]>([]);
  const [newAnchor, setNewAnchor] = useState("");

  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  /** 加载记忆 / 笔记 / 锚点（笔记与锚点需要工作区） */
  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const mem = await getMemory();
      setMemoryPath(mem.path);
      setMemory(mem.content);
      if (workspace && workspace.trim()) {
        const [n, a] = await Promise.all([getNotes(workspace), getAnchors(workspace)]);
        setNotesPath(n.path);
        setNotes(n.items);
        setAnchorsPath(a.path);
        setAnchors(a.items);
      }
      setMsg(null);
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [workspace]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 保存全局记忆 */
  const persistMemory = async () => {
    setBusy(true);
    try {
      await saveMemory(memory);
      setMsg(zh ? "记忆已保存" : "Memory saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** 保存笔记列表 */
  const persistNotes = async (items: string[]) => {
    if (!workspace) return;
    setBusy(true);
    try {
      await saveNotes(workspace, items);
      setNotes(items);
      setMsg(zh ? "笔记已保存" : "Notes saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** 保存锚点列表 */
  const persistAnchors = async (items: string[]) => {
    if (!workspace) return;
    setBusy(true);
    try {
      await saveAnchors(workspace, items);
      setAnchors(items);
      setMsg(zh ? "锚点已保存" : "Anchors saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">
          {zh ? "记忆 / 笔记 / 锚点仅在桌面版可用。" : "Desktop only."}
        </p>
      </div>
    );
  }

  const noWorkspace = !workspace || !workspace.trim();

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">{zh ? "记忆 / 笔记 / 锚点" : "Memory / Notes / Anchors"}</h3>
      <p className="settings-section-desc">
        {zh
          ? "记忆为全局长期事实；笔记按工作区主动查阅；锚点在每次压缩后自动重注入。"
          : "Memory is global; notes are workspace lookups; anchors survive compaction."}
      </p>
      {msg && <p className="settings-hint">{msg}</p>}

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : (
        <>
          {/* 全局记忆 */}
          <h4 className="adv-form-title">{zh ? "用户记忆 (memory)" : "User memory"}</h4>
          {memoryPath && <p className="cfg-tip">{memoryPath}</p>}
          <textarea
            className="cfg-input mem-textarea"
            rows={6}
            placeholder={zh ? "长期记住的事实，每行一条…" : "Long-term facts…"}
            value={memory}
            onChange={(e) => setMemory(e.target.value)}
          />
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => void persistMemory()}
          >
            {zh ? "保存记忆" : "Save memory"}
          </button>

          {/* 工作区笔记 */}
          <h4 className="adv-form-title mem-section-gap">{zh ? "工作区笔记 (note)" : "Workspace notes"}</h4>
          {noWorkspace ? (
            <p className="adv-empty">{zh ? "请先打开工作区文件夹" : "Open a workspace first"}</p>
          ) : (
            <>
              {notesPath && <p className="cfg-tip">{notesPath}</p>}
              {notes.length === 0 ? (
                <p className="adv-empty">{zh ? "暂无笔记" : "No notes"}</p>
              ) : (
                <ul className="adv-list">
                  {notes.map((note, i) => (
                    <li key={i} className="adv-list-item mem-entry">
                      <pre className="mem-entry-text">{note}</pre>
                      <button
                        type="button"
                        className="btn btn-mini"
                        disabled={busy}
                        onClick={() => void persistNotes(notes.filter((_, idx) => idx !== i))}
                      >
                        {zh ? "删除" : "Delete"}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <div className="adv-form-row">
                <input
                  className="cfg-input"
                  placeholder={zh ? "新增笔记…" : "New note…"}
                  value={newNote}
                  onChange={(e) => setNewNote(e.target.value)}
                />
                <button
                  type="button"
                  className="btn btn-mini"
                  disabled={busy}
                  onClick={() => {
                    const v = newNote.trim();
                    if (!v) return;
                    void persistNotes([...notes, v]);
                    setNewNote("");
                  }}
                >
                  +
                </button>
              </div>
            </>
          )}

          {/* 工作区锚点 */}
          <h4 className="adv-form-title mem-section-gap">{zh ? "锚点 (anchor)" : "Anchors"}</h4>
          {noWorkspace ? (
            <p className="adv-empty">{zh ? "请先打开工作区文件夹" : "Open a workspace first"}</p>
          ) : (
            <>
              {anchorsPath && <p className="cfg-tip">{anchorsPath}</p>}
              {anchors.length === 0 ? (
                <p className="adv-empty">{zh ? "暂无锚点" : "No anchors"}</p>
              ) : (
                <ul className="adv-list">
                  {anchors.map((anchor, i) => (
                    <li key={i} className="adv-list-item mem-entry">
                      <pre className="mem-entry-text">{anchor}</pre>
                      <button
                        type="button"
                        className="btn btn-mini"
                        disabled={busy}
                        onClick={() => void persistAnchors(anchors.filter((_, idx) => idx !== i))}
                      >
                        {zh ? "删除" : "Delete"}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <div className="adv-form-row">
                <input
                  className="cfg-input"
                  placeholder={zh ? "新增锚点（压缩后自动保留）…" : "New anchor…"}
                  value={newAnchor}
                  onChange={(e) => setNewAnchor(e.target.value)}
                />
                <button
                  type="button"
                  className="btn btn-mini"
                  disabled={busy}
                  onClick={() => {
                    const v = newAnchor.trim();
                    if (!v) return;
                    void persistAnchors([...anchors, v]);
                    setNewAnchor("");
                  }}
                >
                  +
                </button>
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
