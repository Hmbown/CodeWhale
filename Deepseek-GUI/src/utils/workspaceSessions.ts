// 项目（工作区）与会话绑定：打开文件夹时恢复上次 Agent，无则新建（类似 Cursor）

import type { ThreadRecord } from "../api/types";
import { normalizeWorkspacePath, workspacePathsEqual } from "./workspacePaths";

const STORAGE_KEY = "ds_workspace_threads";

/** 工作区路径 → 上次使用的 threadId */
type WorkspaceThreadMap = Record<string, string>;

/** 读取持久化的「项目 → 会话」映射 */
function loadMap(): WorkspaceThreadMap {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as WorkspaceThreadMap;
    }
  } catch {
    /* 忽略损坏数据 */
  }
  return {};
}

/** 持久化映射 */
function saveMap(map: WorkspaceThreadMap): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
}

/** 记录某项目上次使用的会话 id */
export function saveLastThreadForWorkspace(workspace: string, threadId: string): void {
  if (!workspace.trim() || !threadId.trim()) return;
  const key = normalizeWorkspacePath(workspace);
  const map = loadMap();
  map[key] = threadId;
  saveMap(map);
}

/** 读取某项目上次使用的会话 id */
export function getSavedThreadId(workspace: string): string | null {
  const key = normalizeWorkspacePath(workspace);
  return loadMap()[key] ?? null;
}

/** 筛选属于指定工作区的会话，按 updated_at 降序 */
export function filterThreadsForWorkspace(
  threads: ThreadRecord[],
  workspace: string,
  includeArchived = false,
): ThreadRecord[] {
  return threads
    .filter((t) => workspacePathsEqual(t.workspace, workspace))
    .filter((t) => includeArchived || !t.archived)
    .sort(
      (a, b) =>
        new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
    );
}

/**
 * 为打开的项目挑选应激活的会话：
 * 1. 优先上次使用的会话（仍存在且未归档）
 * 2. 否则选该项目最近更新的会话
 */
export function pickThreadForWorkspace(
  threads: ThreadRecord[],
  workspace: string,
): ThreadRecord | null {
  const candidates = filterThreadsForWorkspace(threads, workspace, false);
  if (candidates.length === 0) return null;

  const savedId = getSavedThreadId(workspace);
  if (savedId) {
    const saved = candidates.find((t) => t.id === savedId);
    if (saved) return saved;
  }
  return candidates[0];
}
