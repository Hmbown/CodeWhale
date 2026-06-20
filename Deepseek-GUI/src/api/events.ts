// SSE 事件流客户端
// 采用 fetch + ReadableStream 解析，便于附加鉴权头并支持 since_seq 断点续传与自动重连

import type { RuntimeEvent } from "./types";
import type { ClientConfig } from "./client";

/** 事件回调签名 */
export type EventHandler = (event: RuntimeEvent) => void;

/** 连接状态回调 */
export type StatusHandler = (connected: boolean) => void;

/**
 * 订阅某个线程的事件流。
 * 返回一个取消函数；内部会在断线后自动以最新 seq 重连。
 */
export function subscribeThreadEvents(
  cfg: ClientConfig,
  threadId: string,
  sinceSeq: number,
  onEvent: EventHandler,
  onStatus?: StatusHandler,
): () => void {
  let aborted = false;
  let controller: AbortController | null = null;
  let lastSeq = sinceSeq;

  const connect = async () => {
    while (!aborted) {
      controller = new AbortController();
      try {
        const url = `${cfg.baseUrl}/v1/threads/${threadId}/events?since_seq=${lastSeq}`;
        const headers: Record<string, string> = { Accept: "text/event-stream" };
        if (cfg.token) headers["Authorization"] = `Bearer ${cfg.token}`;

        const res = await fetch(url, { headers, signal: controller.signal });
        if (!res.ok || !res.body) {
          throw new Error(`SSE HTTP ${res.status}`);
        }
        onStatus?.(true);

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";

        while (!aborted) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });

          // 按 SSE 规范以空行分隔事件块
          let idx: number;
          while ((idx = buffer.indexOf("\n\n")) !== -1) {
            const rawBlock = buffer.slice(0, idx);
            buffer = buffer.slice(idx + 2);
            const evt = parseSseBlock(rawBlock);
            if (evt) {
              lastSeq = Math.max(lastSeq, evt.seq);
              onEvent(evt);
            }
          }
        }
      } catch (err) {
        if (aborted) break;
        // 重连前提示断开
        onStatus?.(false);
      }
      // 断线后短暂等待再重连
      if (!aborted) await sleep(1000);
    }
  };

  connect();

  return () => {
    aborted = true;
    controller?.abort();
  };
}

/** 解析单个 SSE 事件块，提取 data 行并反序列化为 RuntimeEvent */
function parseSseBlock(block: string): RuntimeEvent | null {
  const dataLines: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("data:")) {
      dataLines.push(line.slice(5).trimStart());
    }
    // 忽略 event: / id: / keepalive 注释行（以 ':' 开头）
  }
  if (dataLines.length === 0) return null;
  const payload = dataLines.join("\n");
  try {
    return JSON.parse(payload) as RuntimeEvent;
  } catch {
    return null;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
