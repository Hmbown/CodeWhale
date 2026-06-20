// 编辑器 LSP Hook：为 CodeMirror 注入 language server 补全/悬停/诊断扩展

import type { Extension } from "@codemirror/state";
import { useEffect, useState } from "react";
import { isTauri, formatInvokeError } from "../api/tauri";
import { getLspEditorExtensions, releaseLspEditorExtensions } from "../lsp/lspSessionPool";

/** 为当前文件挂载 LSP 扩展；非 Tauri 或无 server 时返回空 */
export function useEditorLsp(
  workspaceRoot: string | null | undefined,
  filePath: string,
  enabled: boolean,
) {
  const [lspExtensions, setLspExtensions] = useState<Extension[]>([]);
  /** LSP 不可用时的提示（如未安装 rust-analyzer） */
  const [lspHint, setLspHint] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled || !isTauri() || !workspaceRoot?.trim() || !filePath) {
      setLspExtensions([]);
      setLspHint(null);
      return;
    }

    let alive = true;
    void (async () => {
      try {
        const exts = await getLspEditorExtensions(workspaceRoot, filePath);
        if (!alive) return;
        setLspExtensions(exts);
        setLspHint(exts.length === 0 ? null : null);
      } catch (err) {
        if (!alive) return;
        setLspExtensions([]);
        setLspHint(formatInvokeError(err));
      }
    })();

    return () => {
      alive = false;
      releaseLspEditorExtensions(workspaceRoot, filePath);
      setLspExtensions([]);
    };
  }, [workspaceRoot, filePath, enabled]);

  return { lspExtensions, lspHint };
}
