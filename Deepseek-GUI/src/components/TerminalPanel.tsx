// 集成终端面板：Tauri PTY + xterm.js

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import type { Locale } from "../i18n";
import {
  isTauri,
  ptyClose,
  ptyResize,
  ptySpawn,
  ptyWrite,
} from "../api/tauri";
import "@xterm/xterm/css/xterm.css";

interface TerminalPanelProps {
  locale: Locale;
  /** 工作区 cwd（spawn shell 时使用） */
  workspace: string | null;
  /** 填充模式：用于底部停靠面板，不渲染标题/说明，终端铺满容器 */
  fill?: boolean;
}

/** 桌面集成终端（PowerShell / bash） */
export function TerminalPanel({ locale, workspace, fill = false }: TerminalPanelProps) {
  const zh = locale === "zh";
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!isTauri() || !hostRef.current) return;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "Consolas, 'Courier New', monospace",
      theme: {
        background: "#1e1e1e",
        foreground: "#d4d4d4",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    let unlisten: (() => void) | undefined;
    let disposed = false;

    const onResize = () => {
      fit.fit();
      const pid = ptyIdRef.current;
      if (pid && termRef.current) {
        void ptyResize(pid, term.cols, term.rows);
      }
    };
    window.addEventListener("resize", onResize);
    // 容器尺寸变化（底部面板拖动高度/显示隐藏）时自动重排终端列宽行高
    const ro = new ResizeObserver(() => onResize());
    if (hostRef.current) ro.observe(hostRef.current);

    const boot = async () => {
      try {
        fit.fit();
        const id = await ptySpawn({
          cwd: workspace?.trim() || undefined,
          cols: term.cols,
          rows: term.rows,
        });
        if (disposed) {
          await ptyClose(id);
          return;
        }
        ptyIdRef.current = id;
        setReady(true);

        unlisten = await listen<{ id: string; data: string }>("pty-output", (ev) => {
          if (ev.payload.id === ptyIdRef.current) {
            term.write(ev.payload.data);
          }
        });

        term.onData((data) => {
          const pid = ptyIdRef.current;
          if (pid) void ptyWrite(pid, data);
        });
      } catch (e) {
        setErr((e as Error).message);
      }
    };

    void boot();

    return () => {
      disposed = true;
      window.removeEventListener("resize", onResize);
      ro.disconnect();
      unlisten?.();
      const pid = ptyIdRef.current;
      if (pid) void ptyClose(pid);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      ptyIdRef.current = null;
    };
  }, [workspace]);

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">{zh ? "终端仅在桌面版可用。" : "Desktop only."}</p>
      </div>
    );
  }

  // 填充模式：用于底部停靠面板，终端铺满容器，仅在出错/连接时显示一行提示
  if (fill) {
    return (
      <div className="terminal-fill">
        {err && <p className="settings-hint settings-hint-error">{err}</p>}
        {!ready && !err && <p className="adv-list-meta">{zh ? "连接中…" : "Connecting…"}</p>}
        <div className="terminal-host terminal-host-fill" ref={hostRef} />
      </div>
    );
  }

  return (
    <div className="settings-section adv-settings terminal-panel">
      <h3 className="settings-section-title">{zh ? "终端" : "Terminal"}</h3>
      <p className="settings-section-desc">
        {zh
          ? "本地 PTY shell（工作区作为 cwd）。"
          : "Local PTY shell (workspace as cwd)."}
      </p>
      {err && <p className="settings-hint settings-hint-error">{err}</p>}
      {!ready && !err && <p className="adv-list-meta">{zh ? "连接中…" : "Connecting…"}</p>}
      <div className="terminal-host" ref={hostRef} />
    </div>
  );
}
