// 多线程会话缓存：切换标签时保留消息与 latest_seq，避免重复 SSE 全量回放

import type { UiItem, PendingApproval } from "../state/useConversation";

/** 单个线程的会话快照 */
export interface ThreadConvSnapshot {
  /** 消息列表（与 itemMap 顺序一致） */
  items: UiItem[];
  /** 按 item_id 索引的消息映射 */
  itemMap: Map<string, UiItem>;
  /** 已消费的最大事件 seq */
  latestSeq: number;
  running: boolean;
  currentTurnId: string | null;
  approvals: PendingApproval[];
  usageTick: number;
}

/** 进程内全局缓存（Map：threadId → 快照） */
const caches = new Map<string, ThreadConvSnapshot>();

/** 读取某线程的缓存快照 */
export function getThreadConvCache(threadId: string): ThreadConvSnapshot | undefined {
  const hit = caches.get(threadId);
  if (!hit) return undefined;
  // 返回深拷贝，避免外部 mutate 污染缓存
  return {
    items: [...hit.items],
    itemMap: new Map(hit.itemMap),
    latestSeq: hit.latestSeq,
    running: hit.running,
    currentTurnId: hit.currentTurnId,
    approvals: [...hit.approvals],
    usageTick: hit.usageTick,
  };
}

/** 写入/更新某线程的缓存快照 */
export function setThreadConvCache(threadId: string, snap: ThreadConvSnapshot): void {
  caches.set(threadId, {
    items: [...snap.items],
    itemMap: new Map(snap.itemMap),
    latestSeq: snap.latestSeq,
    running: snap.running,
    currentTurnId: snap.currentTurnId,
    approvals: [...snap.approvals],
    usageTick: snap.usageTick,
  });
}

/** 删除某线程缓存（如归档后不再需要） */
export function deleteThreadConvCache(threadId: string): void {
  caches.delete(threadId);
}

/** 清空全部缓存（如切换工作区时可选用） */
export function clearAllThreadConvCaches(): void {
  caches.clear();
}
