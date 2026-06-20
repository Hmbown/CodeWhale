// 聊天 transcript 自动滚底：发送后跳到底部，流式输出时若已在底部则跟随

import { useCallback, useEffect, useRef } from "react";

/** 距底部多少像素内视为「在底部」 */
const BOTTOM_THRESHOLD = 96;

/**
 * 管理聊天消息区滚动：发送后强制滚底；流式更新时仅在用户未上滑时跟随。
 * @param items 消息列表（长度或内容变化时触发跟随滚动）
 * @param running 是否正在生成（流式阶段持续跟随）
 * @param threadId 当前线程 id（切换会话时滚到底）
 */
export function useTranscriptAutoScroll(
  items: readonly unknown[],
  running: boolean,
  threadId: string | null,
) {
  const ref = useRef<HTMLDivElement>(null);
  /** 发送/转向后短暂强制滚底，忽略用户上滑状态 */
  const forceScrollRef = useRef(false);

  /** 判断滚动容器是否接近底部 */
  const isNearBottom = useCallback(() => {
    const el = ref.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_THRESHOLD;
  }, []);

  /** 滚到最底部 */
  const scrollToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
    const el = ref.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior });
  }, []);

  /** 用户发送/转向后立即滚底（Composer 提交时调用） */
  const scrollAfterSend = useCallback(() => {
    forceScrollRef.current = true;
    requestAnimationFrame(() => {
      scrollToBottom("smooth");
      // 下一帧再滚一次，避免 DOM 尚未插入用户消息
      requestAnimationFrame(() => {
        scrollToBottom("auto");
        forceScrollRef.current = false;
      });
    });
  }, [scrollToBottom]);

  // 切换会话：打开后展示最新消息
  useEffect(() => {
    if (!threadId) return;
    requestAnimationFrame(() => scrollToBottom("auto"));
  }, [threadId, scrollToBottom]);

  // 消息列表或流式状态变化：在底部或刚发送时跟随
  const tailKey =
    items.length > 0
      ? String((items[items.length - 1] as { text?: string; done?: boolean })?.text?.length ?? 0)
      : "0";

  useEffect(() => {
    if (forceScrollRef.current || isNearBottom() || running) {
      requestAnimationFrame(() => scrollToBottom("auto"));
    }
  }, [items.length, tailKey, running, isNearBottom, scrollToBottom]);

  return { ref, scrollAfterSend, scrollToBottom };
}
