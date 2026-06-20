// 单条消息渲染组件：按 TurnItemKind 展示不同样式
// 推理块（agent_reasoning）折叠并视觉区分，工具/命令/文件变更用代码块样式

import { useEffect, useState } from "react";
import type { UiItem } from "../state/useConversation";
import { Markdown } from "./Markdown";
import { parsePathsFromDiff } from "../utils/workspacePaths";
import { translateRuntimeStatus } from "../i18n/runtimeStatus";
import type { Locale } from "../i18n";

/** 各类型对应的中文标签 */
const KIND_LABEL: Record<string, string> = {
  user_message: "我",
  agent_message: "助手",
  agent_reasoning: "推理",
  tool_call: "工具调用",
  file_change: "文件变更",
  command_execution: "命令执行",
  context_compaction: "上下文压缩",
  status: "状态",
  error: "错误",
};

/** 工具/命令类图标 */
const KIND_ICON: Record<string, string> = {
  tool_call: "🔧",
  command_execution: "▶",
  file_change: "📝",
};

export function MessageItem({
  item,
  locale = "zh",
  onOpenFile,
}: {
  item: UiItem;
  /** 界面语言，用于翻译后端 status 英文消息 */
  locale?: Locale;
  /** 点击文件变更卡片时打开对应文件 */
  onOpenFile?: (path: string) => void;
}) {
  const isTool =
    item.kind === "tool_call" ||
    item.kind === "command_execution" ||
    item.kind === "file_change";
  // 工具/命令/文件卡片与推理块默认折叠，其余展开
  const [expanded, setExpanded] = useState(item.kind !== "agent_reasoning" && !isTool);

  // 工具类用专门的折叠卡片渲染
  if (isTool) {
    return (
      <ToolCard
        item={item}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
        onOpenFile={onOpenFile}
      />
    );
  }

  // 推理（思考）块用专门的思考卡片渲染
  if (item.kind === "agent_reasoning") {
    return <ThinkingCard item={item} />;
  }

  // 用户消息：Cursor 风格圆角 pill（无「我」标签）
  if (item.kind === "user_message") {
    return <UserMessagePill item={item} />;
  }

  const label = KIND_LABEL[item.kind] ?? item.kind;
  const bodyText =
    item.kind === "status" ? translateRuntimeStatus(item.text, locale) : item.text;
  const cls = [
    "msg",
    `msg-${item.kind}`,
    item.done ? "msg-done" : "msg-streaming",
    item.failed ? "msg-failed" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const isMarkdown = item.kind === "agent_message";

  return (
    <div className={cls}>
      <div className="msg-head">
        <span className="msg-label">{label}</span>
        {!item.done && <span className="msg-cursor">▍</span>}
      </div>
      {expanded &&
        (isMarkdown && bodyText ? (
          <Markdown text={bodyText} />
        ) : (
          <div className="msg-body">{bodyText || "…"}</div>
        ))}
    </div>
  );
}

/** 用户消息 pill：单行/多行文本 + 右侧 ↵ 图标（仿 Cursor） */
function UserMessagePill({ item }: { item: UiItem }) {
  return (
    <div className={`user-msg-pill${item.done ? "" : " user-msg-streaming"}`}>
      <div className="user-msg-text">{item.text || "…"}</div>
      <span className="user-msg-icon" aria-hidden title="已发送">
        ↵
      </span>
    </div>
  );
}

/** 推理（思考）块：流式时展开并实时计时，完成后折叠并显示思考用时 */
function ThinkingCard({ item }: { item: UiItem }) {
  // 流式中默认展开让用户看到思考过程，完成后默认折叠
  const [expanded, setExpanded] = useState(!item.done);
  // 落定时自动折叠
  useEffect(() => {
    if (item.done) setExpanded(false);
  }, [item.done]);

  // 流式中实时刷新计时
  const [, force] = useState(0);
  useEffect(() => {
    if (item.done) return;
    const t = setInterval(() => force((n) => n + 1), 300);
    return () => clearInterval(t);
  }, [item.done]);

  const elapsedMs = item.done
    ? item.durationMs ?? 0
    : item.startedAt
      ? Date.now() - item.startedAt
      : 0;
  const secs = (elapsedMs / 1000).toFixed(1);

  return (
    <div className={`think-card ${item.done ? "think-done" : "think-streaming"}`}>
      <div className="think-head" onClick={() => setExpanded((v) => !v)}>
        <span className="think-caret">{expanded ? "▾" : "▸"}</span>
        <span className="think-icon">🧠</span>
        <span className="think-label">
          {item.done ? `已思考 ${secs}s` : "思考中"}
          {!item.done && <span className="think-dots" />}
        </span>
        {!item.done && <span className="think-timer">{secs}s</span>}
      </div>
      {expanded && item.text && (
        <div className="think-body">{item.text}</div>
      )}
    </div>
  );
}

/** 工具调用 / 命令执行 / 文件变更：带状态徽标的折叠卡片 */
function ToolCard({
  item,
  expanded,
  onToggle,
  onOpenFile,
}: {
  item: UiItem;
  expanded: boolean;
  onToggle: () => void;
  onOpenFile?: (path: string) => void;
}) {
  const icon = KIND_ICON[item.kind] ?? "🔧";
  const label = KIND_LABEL[item.kind] ?? item.kind;
  const name = item.title || label;

  // 状态：进行中 / 失败 / 成功
  const status = !item.done ? "running" : item.failed ? "failed" : "done";
  const statusText = status === "running" ? "运行中" : status === "failed" ? "失败" : "完成";

  const isDiff = item.kind === "file_change";
  /** 文件变更卡片：点击标题可在编辑器中打开首个变更文件 */
  const openChangedFile = () => {
    if (!isDiff || !onOpenFile) return;
    const p = item.filePaths?.[0] ?? parsePathsFromDiff(item.text)[0];
    if (p) onOpenFile(p);
  };

  return (
    <div className={`tool-card tool-${status}${isDiff && onOpenFile ? " tool-clickable" : ""}`}>
      <div
        className="tool-card-head"
        onClick={() => {
          if (isDiff && onOpenFile && (item.filePaths?.[0] || item.text.includes("+++"))) {
            openChangedFile();
          } else {
            onToggle();
          }
        }}
        title={isDiff && onOpenFile ? "点击在编辑器中打开此文件" : undefined}
      >
        <span className="tool-caret" onClick={(e) => { e.stopPropagation(); onToggle(); }}>
          {expanded ? "▾" : "▸"}
        </span>
        <span className="tool-icon">{icon}</span>
        <span className="tool-name" title={name}>
          {name}
        </span>
        <span className={`tool-status tool-status-${status}`}>{statusText}</span>
      </div>
      {expanded && item.text && (
        <div className="tool-card-body">
          {isDiff ? (
            <DiffBody text={item.text} />
          ) : (
            <pre className="tool-pre">{item.text}</pre>
          )}
        </div>
      )}
    </div>
  );
}

/** 文件变更 diff 渲染：按行着色（+ 新增 / - 删除 / @@ 区块头） */
function DiffBody({ text }: { text: string }) {
  const lines = (text || "").split("\n");
  return (
    <div className="msg-body msg-mono diff">
      {lines.map((ln, i) => {
        let cls = "diff-line";
        if (ln.startsWith("+") && !ln.startsWith("+++")) cls += " diff-add";
        else if (ln.startsWith("-") && !ln.startsWith("---")) cls += " diff-del";
        else if (ln.startsWith("@@")) cls += " diff-hunk";
        else if (ln.startsWith("diff ") || ln.startsWith("+++") || ln.startsWith("---"))
          cls += " diff-meta";
        return (
          <div key={i} className={cls}>
            {ln || " "}
          </div>
        );
      })}
    </div>
  );
}
