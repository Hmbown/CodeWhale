// 编辑命令总线：让顶部「编辑/选择」菜单的撤销/重做/剪切/复制/粘贴/全选/查找
// 作用到「当前聚焦的目标」——可能是 CodeMirror 代码编辑器，也可能是普通
// <input>/<textarea>（如 Composer 输入框）。
//
// 设计要点：
// 1. 点击菜单会让焦点转移到菜单按钮，因此不能在执行命令时才读 document.activeElement，
//    而要在 focusin 阶段提前记住「最后一个可编辑目标」。
// 2. CodeMirror 的内容区是 contenteditable，通过已注册的视图集合判断焦点归属，
//    从而调用 CM 原生命令（undo/redo/selectAll/search），保证与编辑器自身历史一致。
// 3. 普通输入框走 document.execCommand / navigator.clipboard，兼容 WebView2。

import { undo, redo, selectAll, selectParentSyntax } from "@codemirror/commands";
import { openSearchPanel } from "@codemirror/search";
import type { EditorView } from "@codemirror/view";

/** 已挂载的 CodeMirror 视图集合（CodeView 创建/卸载时增删） */
const views = new Set<EditorView>();
/** 最近聚焦的 CodeMirror 视图 */
let lastView: EditorView | null = null;
/** 最近聚焦的普通可编辑元素（input/textarea/contenteditable） */
let lastEditable: HTMLElement | null = null;
/** 全局 focusin 监听是否已安装 */
let listenerInstalled = false;

/** 判断元素是否为普通可编辑控件 */
function isPlainEditable(el: EventTarget | null): el is HTMLElement {
  if (!(el instanceof HTMLElement)) return false;
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) return true;
  return el.isContentEditable;
}

/** 安装一次性的全局焦点跟踪 */
function ensureListener() {
  if (listenerInstalled || typeof window === "undefined") return;
  listenerInstalled = true;
  document.addEventListener(
    "focusin",
    (e) => {
      const target = e.target as HTMLElement | null;
      if (!target) return;
      // 优先判断是否落在某个 CodeMirror 视图内
      for (const v of views) {
        if (v.dom.contains(target)) {
          lastView = v;
          lastEditable = null;
          return;
        }
      }
      if (isPlainEditable(target)) {
        // 忽略菜单内部的按钮等非真正编辑目标
        lastEditable = target;
        lastView = null;
      }
    },
    true,
  );
}

/** CodeView 创建编辑器时注册视图 */
export function registerEditorView(view: EditorView) {
  ensureListener();
  views.add(view);
}

/** CodeView 卸载时注销视图 */
export function unregisterEditorView(view: EditorView) {
  views.delete(view);
  if (lastView === view) lastView = null;
}

/** 读取剪贴板文本（优先 navigator.clipboard，失败返回空串） */
async function readClipboard(): Promise<string> {
  try {
    if (navigator.clipboard?.readText) {
      return await navigator.clipboard.readText();
    }
  } catch {
    // 权限不足或非安全上下文时回退
  }
  return "";
}

/** 撤销：作用于当前聚焦的编辑器/输入框 */
export function editUndo() {
  if (lastView) {
    lastView.focus();
    undo(lastView);
    return;
  }
  if (lastEditable) {
    lastEditable.focus();
    document.execCommand("undo");
  }
}

/** 重做 */
export function editRedo() {
  if (lastView) {
    lastView.focus();
    redo(lastView);
    return;
  }
  if (lastEditable) {
    lastEditable.focus();
    // 不同浏览器实现差异：先试 redo，再退回 insertText 无效则忽略
    document.execCommand("redo");
  }
}

/** 全选 */
export function editSelectAll() {
  if (lastView) {
    lastView.focus();
    selectAll(lastView);
    return;
  }
  if (lastEditable) {
    lastEditable.focus();
    document.execCommand("selectAll");
  }
}

/** 扩展选择（CodeMirror 按语法向上扩展；普通输入框退回全选） */
export function editExpandSelection() {
  if (lastView) {
    lastView.focus();
    selectParentSyntax(lastView);
    return;
  }
  if (lastEditable) {
    lastEditable.focus();
    document.execCommand("selectAll");
  }
}

/** 复制当前选中内容 */
export function editCopy() {
  if (lastView) lastView.focus();
  else if (lastEditable) lastEditable.focus();
  else return;
  document.execCommand("copy");
}

/** 剪切当前选中内容 */
export function editCut() {
  if (lastView) lastView.focus();
  else if (lastEditable) lastEditable.focus();
  else return;
  document.execCommand("cut");
}

/** 粘贴剪贴板文本到当前光标处 */
export async function editPaste() {
  const text = await readClipboard();
  if (!text) {
    // 退回浏览器原生粘贴（部分 WebView 仍可用）
    if (lastView) lastView.focus();
    else if (lastEditable) lastEditable.focus();
    document.execCommand("paste");
    return;
  }
  if (lastView) {
    lastView.focus();
    // 用 CM 事务替换当前选区，保证进入编辑器历史
    lastView.dispatch(lastView.state.replaceSelection(text));
    return;
  }
  if (lastEditable) {
    lastEditable.focus();
    // execCommand insertText 会触发 React 的受控 onChange
    document.execCommand("insertText", false, text);
  }
}

/** 在当前编辑器中查找（CodeMirror 搜索面板） */
export function editFind(): boolean {
  if (lastView) {
    lastView.focus();
    openSearchPanel(lastView);
    return true;
  }
  return false;
}

/** 是否有可用的代码编辑器焦点（用于菜单项启用判断） */
export function hasEditorFocus(): boolean {
  return lastView != null;
}
