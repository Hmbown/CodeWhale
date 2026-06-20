// LSP 会话池：按 workspace+语言 复用 LSPClient 与 language server 进程

import {
  LSPClient,
  languageServerExtensions,
  type Transport,
} from "@codemirror/lsp-client";
import type { Extension } from "@codemirror/state";
import { lspStartSession, lspStopSession } from "../api/tauri";
import { hasLspServer, languageIdFromPath } from "./languageId";
import { pathToFileUri } from "./pathToUri";
import { TauriLspTransport } from "./tauriTransport";

/** 池内条目 */
interface PoolEntry {
  client: LSPClient;
  transport: TauriLspTransport;
  sessionId: string;
  refCount: number;
  ready: Promise<void>;
}

const pool = new Map<string, PoolEntry>();

/** 池键：workspace + 语言（JS 与 TS 共用 typescript server） */
function poolKey(workspace: string, languageId: string): string {
  const lang = languageId === "javascript" ? "typescript" : languageId;
  return `${workspace.toLowerCase()}::${lang}`;
}

/** 获取或创建 LSP 会话，返回 CodeMirror 扩展（含补全/悬停/诊断） */
export async function getLspEditorExtensions(
  workspace: string,
  filePath: string,
): Promise<Extension[]> {
  const languageId = languageIdFromPath(filePath);
  if (!languageId || !hasLspServer(languageId)) {
    return [];
  }

  const uri = pathToFileUri(filePath);
  const key = poolKey(workspace, languageId);
  let entry = pool.get(key);

  if (!entry) {
    const info = await lspStartSession(workspace, filePath);
    const transport = new TauriLspTransport(info.sessionId);
    await transport.attach();

    const client = new LSPClient({
      rootUri: info.rootUri || pathToFileUri(workspace),
      timeout: 20_000,
      extensions: languageServerExtensions(),
    });
    client.connect(transport as Transport);

    const ready = client.initializing.then(() => undefined).catch(() => undefined);
    entry = {
      client,
      transport,
      sessionId: info.sessionId,
      refCount: 0,
      ready,
    };
    pool.set(key, entry);
  }

  entry.refCount += 1;
  await entry.ready;

  return [entry.client.plugin(uri, languageId)];
}

/** 释放池引用；引用归零时关闭 LSP 会话 */
export function releaseLspEditorExtensions(workspace: string, filePath: string): void {
  const languageId = languageIdFromPath(filePath);
  if (!languageId) return;
  const key = poolKey(workspace, languageId);
  const entry = pool.get(key);
  if (!entry) return;

  entry.refCount -= 1;
  if (entry.refCount > 0) return;

  entry.client.disconnect();
  entry.transport.dispose();
  void lspStopSession(entry.sessionId);
  pool.delete(key);
}
