// 全量变更查看（/diff）：以模态框展示工作区 git diff，按行着色。
// 通过 Tauri 本地命令 git_diff 获取（未暂存 + 已暂存）。

import { useEffect, useState } from "react";
import { gitDiff } from "../api/tauri";

interface DiffModalProps {
  /** 工作区根路径 */
  rootPath: string | null;
  onClose: () => void;
}

/**
 * Git 变更查看模态框。
 * @param rootPath 当前工作区路径
 * @param onClose 关闭回调
 */
export function DiffModal({ rootPath, onClose }: DiffModalProps) {
  const [text, setText] = useState<string>("加载中…");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const out = await gitDiff(rootPath ?? "");
        if (alive) setText(out);
      } catch (e) {
        if (alive) setError((e as Error).message);
      }
    })();
    return () => {
      alive = false;
    };
  }, [rootPath]);

  const lines = (text || "").split("\n");

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-panel diff-panel" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span className="pane-title">工作区变更 (git diff)</span>
          <button className="btn-mini" onClick={onClose}>
            关闭
          </button>
        </div>
        <div className="modal-body">
          {error ? (
            <div className="banner banner-warn">{error}</div>
          ) : (
            <div className="msg-body msg-mono diff">
              {lines.map((ln, i) => {
                let cls = "diff-line";
                if (ln.startsWith("+") && !ln.startsWith("+++")) cls += " diff-add";
                else if (ln.startsWith("-") && !ln.startsWith("---")) cls += " diff-del";
                else if (ln.startsWith("@@")) cls += " diff-hunk";
                else if (ln.startsWith("#")) cls += " diff-hunk";
                else if (
                  ln.startsWith("diff ") ||
                  ln.startsWith("+++") ||
                  ln.startsWith("---")
                )
                  cls += " diff-meta";
                return (
                  <div key={i} className={cls}>
                    {ln || " "}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
