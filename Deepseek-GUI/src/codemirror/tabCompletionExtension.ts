// CodeMirror 扩展：Cursor Tab 风格 AI 内联补全（幽灵文本 + Tab 接受）

import { Facet, Prec, StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration,
  DecorationSet,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
  type ViewUpdate,
} from "@codemirror/view";
import { fetchTabCompletion } from "../utils/tabCompletionService";
import {
  cursorInComment,
  loadTabCompletionSettings,
  pathMatchesIgnoredGlobs,
  TAB_SETTINGS_CHANGE_EVENT,
  type TabCompletionSettings,
} from "../utils/tabCompletionSettings";
import { languageIdFromPath } from "../lsp/languageId";

/** 当前编辑文件路径（用于 API 与忽略 glob） */
export const tabCompletionFilePath = Facet.define<string, string>({
  combine: (values) => values[values.length - 1] ?? "",
});

/** 内联建议状态 */
interface TabSuggestion {
  /** 插入点（光标） */
  from: number;
  /** 待插入全文 */
  text: string;
  /** 已部分接受的字节偏移 */
  partialOffset: number;
}

const setTabSuggestion = StateEffect.define<TabSuggestion | null>();

/** 存储当前幽灵补全 */
const tabSuggestionField = StateField.define<TabSuggestion | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) {
      if (e.is(setTabSuggestion)) return e.value;
    }
    if (tr.docChanged) return null;
    return value;
  },
});

/** 幽灵文本 Widget */
class GhostTextWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }

  eq(other: GhostTextWidget): boolean {
    return other.text === this.text;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-tab-ghost-text";
    span.textContent = this.text;
    span.setAttribute("aria-hidden", "true");
    return span;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/** 根据建议状态生成装饰 */
function buildDecorations(sug: TabSuggestion | null): DecorationSet {
  if (!sug) return Decoration.none;
  const rest = sug.text.slice(sug.partialOffset);
  if (!rest) return Decoration.none;
  return Decoration.set([
    Decoration.widget({
      widget: new GhostTextWidget(rest),
      side: 1,
    }).range(sug.from),
  ]);
}

const tabDecorationField = StateField.define<DecorationSet>({
  create: (state) => buildDecorations(state.field(tabSuggestionField)),
  update(deco, tr) {
    const sug = tr.state.field(tabSuggestionField);
    if (tr.docChanged || tr.effects.some((e) => e.is(setTabSuggestion))) {
      return buildDecorations(sug);
    }
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** 提取光标前后上下文 */
function contextAroundCursor(view: EditorView): {
  from: number;
  prefix: string;
  suffix: string;
  lineText: string;
  col: number;
} | null {
  const sel = view.state.selection.main;
  if (!sel.empty) return null;
  const from = sel.head;
  const doc = view.state.doc;
  const line = doc.lineAt(from);
  return {
    from,
    prefix: doc.sliceString(0, from),
    suffix: doc.sliceString(from),
    lineText: line.text,
    col: from - line.from,
  };
}

/** 是否应跳过本次补全请求 */
function shouldSkip(
  settings: TabCompletionSettings,
  filePath: string,
  lineText: string,
  col: number,
): boolean {
  if (!settings.enabled) return true;
  if (filePath && pathMatchesIgnoredGlobs(filePath, settings.ignoredGlobs)) return true;
  if (!settings.suggestInComments && cursorInComment(lineText, col)) return true;
  return false;
}

/** 过滤空白-only 建议（设置关闭时） */
function filterSuggestion(text: string, settings: TabCompletionSettings): string {
  const trimmed = text.replace(/\r\n/g, "\n");
  if (!settings.whitespaceOnly && trimmed.length > 0 && trimmed.trim().length === 0) {
    return "";
  }
  return trimmed;
}

/** 防抖请求插件 */
function createTabCompletionPlugin(getSettings: () => TabCompletionSettings) {
  return ViewPlugin.fromClass(
    class {
      private timer: ReturnType<typeof setTimeout> | null = null;
      private requestId = 0;

      constructor(private view: EditorView) {
        this.onSettingsChange = this.onSettingsChange.bind(this);
        window.addEventListener(TAB_SETTINGS_CHANGE_EVENT, this.onSettingsChange);
        this.schedule();
      }

      destroy(): void {
        window.removeEventListener(TAB_SETTINGS_CHANGE_EVENT, this.onSettingsChange);
        this.clearTimer();
      }

      private onSettingsChange(): void {
        if (this.view.state.field(tabSuggestionField)) {
          this.view.dispatch({ effects: setTabSuggestion.of(null) });
        }
        this.schedule();
      }

      update(u: ViewUpdate): void {
        if (u.docChanged || u.selectionSet) {
          if (u.startState.field(tabSuggestionField)) {
            this.view.dispatch({ effects: setTabSuggestion.of(null) });
          }
          this.schedule();
        }
      }

      private clearTimer(): void {
        if (this.timer) {
          clearTimeout(this.timer);
          this.timer = null;
        }
      }

      private schedule(): void {
        this.clearTimer();
        this.timer = setTimeout(() => void this.fetchSuggestion(), 450);
      }

      private async fetchSuggestion(): Promise<void> {
        const settings = getSettings();
        const ctx = contextAroundCursor(this.view);
        const filePath = this.view.state.facet(tabCompletionFilePath);
        if (!ctx || shouldSkip(settings, filePath, ctx.lineText, ctx.col)) {
          return;
        }

        const id = ++this.requestId;
        const languageId = languageIdFromPath(filePath) ?? undefined;
        // 按语言决定是否启用自动 import：TS/JS 走 autoImportTs，Python 走 autoImportPy
        const isTsLike =
          languageId === "typescript" || languageId === "javascript";
        const isPy = languageId === "python";
        const autoImport =
          (isTsLike && settings.autoImportTs) || (isPy && settings.autoImportPy);
        const text = await fetchTabCompletion({
          filePath,
          prefix: ctx.prefix,
          suffix: ctx.suffix,
          languageId,
          autoImport,
        });

        if (id !== this.requestId) return;
        const filtered = filterSuggestion(text, settings);
        if (!filtered) return;

        const cur = contextAroundCursor(this.view);
        if (!cur || cur.from !== ctx.from) return;

        this.view.dispatch({
          effects: setTabSuggestion.of({ from: ctx.from, text: filtered, partialOffset: 0 }),
        });
      }
    },
  );
}

/** 接受建议（全文或部分） */
function acceptSuggestion(view: EditorView, partialWord = false): boolean {
  const sug = view.state.field(tabSuggestionField);
  if (!sug) return false;

  const settings = loadTabCompletionSettings();
  let insert = sug.text.slice(sug.partialOffset);
  let newPartial = sug.partialOffset;

  if (partialWord && settings.partialAccept) {
    const m = insert.match(/^(\s*\S+\s*)/);
    if (!m) return false;
    insert = m[1]!;
    newPartial = sug.partialOffset + insert.length;
  } else {
    newPartial = sug.text.length;
  }

  if (!insert) return false;

  view.dispatch({
    changes: { from: sug.from, insert },
    effects:
      newPartial >= sug.text.length
        ? setTabSuggestion.of(null)
        : setTabSuggestion.of({ ...sug, partialOffset: newPartial }),
    selection: { anchor: sug.from + insert.length },
  });
  return true;
}

/** 创建 Tab 补全扩展 */
export function tabCompletionExtension(filePath: string): Extension {
  const settingsRef = { current: loadTabCompletionSettings() };
  const refreshSettings = () => {
    settingsRef.current = loadTabCompletionSettings();
  };
  window.addEventListener(TAB_SETTINGS_CHANGE_EVENT, refreshSettings);

  return [
    tabCompletionFilePath.of(filePath),
    tabSuggestionField,
    tabDecorationField,
    createTabCompletionPlugin(() => settingsRef.current),
    Prec.highest(
      keymap.of([
        {
          key: "Tab",
          run: (view) => acceptSuggestion(view, false),
        },
        {
          key: "Escape",
          run: (view) => {
            const sug = view.state.field(tabSuggestionField);
            if (!sug) return false;
            view.dispatch({ effects: setTabSuggestion.of(null) });
            return true;
          },
        },
        {
          key: "Ctrl-ArrowRight",
          mac: "Cmd-ArrowRight",
          run: (view) => acceptSuggestion(view, true),
        },
      ]),
    ),
  ];
}
