// Tauri 侧 LSP Transport：对接 @codemirror/lsp-client

import type { Transport } from "@codemirror/lsp-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { lspSend } from "../api/tauri";

/** LSP 入站事件名（与 Rust LSP_INBOUND_EVENT 一致） */
export const LSP_INBOUND_EVENT = "lsp-inbound";

interface LspInboundPayload {
  sessionId: string;
  message: string;
}

/** 通过 Tauri invoke + event 实现 LSP JSON-RPC 双向通道 */
export class TauriLspTransport implements Transport {
  private handlers = new Set<(value: string) => void>();
  private unlisten: UnlistenFn | null = null;
  private attached: Promise<void> | null = null;

  /** @param sessionId Rust 侧 LSP 会话 id */
  constructor(private readonly sessionId: string) {}

  /** 订阅 Rust emit 的入站消息（幂等） */
  attach(): Promise<void> {
    if (!this.attached) {
      this.attached = this.doAttach();
    }
    return this.attached;
  }

  private async doAttach(): Promise<void> {
    this.unlisten = await listen<LspInboundPayload>(LSP_INBOUND_EVENT, (ev) => {
      if (ev.payload.sessionId !== this.sessionId) return;
      for (const handler of this.handlers) {
        handler(ev.payload.message);
      }
    });
  }

  /** 发送 JSON-RPC 到 language server（纯 JSON，无 Content-Length 头） */
  send(message: string): void {
    void lspSend(this.sessionId, message);
  }

  subscribe(handler: (value: string) => void): void {
    this.handlers.add(handler);
  }

  unsubscribe(handler: (value: string) => void): void {
    this.handlers.delete(handler);
  }

  /** 释放 event 监听 */
  dispose(): void {
    void this.unlisten?.();
    this.unlisten = null;
    this.handlers.clear();
  }
}
