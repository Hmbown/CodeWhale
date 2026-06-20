// 斜杠命令执行器：将 TUI 常用 / 命令映射到 GUI 操作

import type { RuntimeClient } from "../api/client";
import type { ThreadRecord } from "../api/types";
import type { SettingsTab } from "../components/SettingsView";
import type { Locale } from "../i18n";
import { activateProfile, isTauri, listProfiles, pickFile } from "../api/tauri";
import { formatSlashHelp, parseSlashInput } from "./slashCommands";

/** 斜杠命令执行上下文（由 App 注入） */
export interface SlashCommandContext {
  client: RuntimeClient;
  locale: Locale;
  models: string[];
  modes: string[];
  activeId: string | null;
  activeThread: ThreadRecord | null;
  rootPath: string | null;
  /** 当前会话最后一条用户消息（/retry） */
  lastUserMessage: string | null;
  setShowDiff: (v: boolean) => void;
  setShowSessions: (v: boolean) => void;
  setShowUsage: (v: boolean) => void;
  /** 打开快照还原模态框 */
  setShowSnapshots: (v: boolean) => void;
  /** 还原/撤销成功后回调（刷新文件树等） */
  afterRestore?: () => void;
  openSettings: (tab: SettingsTab) => void;
  onChangeThreadField: (patch: {
    model?: string;
    mode?: string;
    trust_mode?: boolean;
    auto_approve?: boolean;
  }) => Promise<void>;
  refresh: () => Promise<void>;
  setActiveId: (id: string | null) => void;
  openWorkspace: (dir: string) => Promise<void>;
  chooseFolder: () => Promise<void>;
  onSend: (text: string) => Promise<void>;
  /** 向 Composer 插入 @ 路径 */
  insertAttachmentPath: (relPath: string) => void;
  /** 入队一条消息（回合结束后自动发送） */
  enqueue: (text: string) => void;
  /** 清空队列 */
  clearQueue: () => void;
  /** 队列整体停泊到暂存 */
  stashQueue: () => void;
  /** 暂存整体弹回队列 */
  popStash: () => void;
  /** 清空暂存 */
  clearStash: () => void;
}

/** 解析 /mode 参数为合法 mode */
function parseModeArg(arg: string): string | null {
  const a = arg.trim().toLowerCase();
  if (a === "1" || a === "plan") return "plan";
  if (a === "2" || a === "agent") return "agent";
  if (a === "3" || a === "yolo") return "yolo";
  if (["plan", "agent", "yolo"].includes(a)) return a;
  return null;
}

/** 解析 trust on/off */
function parseTrustArg(arg: string): boolean | null {
  const a = arg.trim().toLowerCase();
  if (["on", "true", "1", "yes"].includes(a)) return true;
  if (["off", "false", "0", "no"].includes(a)) return false;
  return null;
}

/** 下载 JSON 到本地（WebView / 浏览器通用） */
function downloadJson(filename: string, data: unknown): void {
  const json = JSON.stringify(data, null, 2);
  const blob = new Blob([json], { type: "application/json;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

/** 执行斜杠命令；返回是否已处理（true = 不应再当普通消息发送） */
export async function executeSlashCommand(
  raw: string,
  ctx: SlashCommandContext,
): Promise<boolean> {
  const text = raw.trim();
  if (!text.startsWith("/")) return false;

  const { cmd, arg } = parseSlashInput(text);

  try {
    switch (cmd) {
      case "help":
        alert(formatSlashHelp(ctx.locale));
        return true;
      case "diff":
        ctx.setShowDiff(true);
        return true;
      case "sessions":
        ctx.setShowSessions(true);
        return true;
      case "task":
      case "tasks":
        ctx.openSettings("tasks");
        return true;
      case "provider":
        ctx.openSettings("models");
        return true;
      case "mcp":
        ctx.openSettings("mcp");
        return true;
      case "hooks":
        ctx.openSettings("hooks");
        return true;
      case "network":
        ctx.openSettings("network");
        return true;
      case "jobs":
        ctx.openSettings("jobs");
        return true;
      case "subagents":
        ctx.openSettings("subagents");
        return true;
      case "memory":
      case "note":
      case "anchor":
        ctx.openSettings("memory");
        return true;
      case "settings":
        ctx.openSettings("models");
        return true;
      case "config":
        ctx.openSettings("connection");
        return true;
      case "load":
        ctx.setShowSessions(true);
        return true;
      case "queue": {
        // /queue <text> 入队；/queue clear 清空；无参时提示
        const sub = arg.trim();
        const low = sub.toLowerCase();
        if (low === "clear") {
          ctx.clearQueue();
          return true;
        }
        if (low === "stash") {
          ctx.stashQueue();
          return true;
        }
        if (sub && low !== "list") {
          ctx.enqueue(sub);
          return true;
        }
        // 无参或 list：队列条已直接展示，给出用法提示
        alert(
          ctx.locale === "zh"
            ? "用法：/queue <消息> 入队；/queue clear 清空；/queue stash 暂存全部"
            : "Usage: /queue <msg> | /queue clear | /queue stash",
        );
        return true;
      }
      case "stash": {
        // /stash 暂存当前队列；/stash pop 弹回；/stash clear 清空
        const low = arg.trim().toLowerCase();
        if (low === "pop") {
          ctx.popStash();
        } else if (low === "clear") {
          ctx.clearStash();
        } else {
          ctx.stashQueue();
        }
        return true;
      }
      case "rlm":
      case "recursive":
        ctx.openSettings("rlm");
        return true;
      case "terminal":
      case "term":
      case "shell":
        ctx.openSettings("terminal");
        return true;
      case "workspace":
        if (arg.trim()) {
          await ctx.openWorkspace(arg.trim());
        } else {
          await ctx.chooseFolder();
        }
        return true;
      case "profile": {
        if (!isTauri()) {
          alert(ctx.locale === "zh" ? "配置档案仅在桌面版可用" : "Profiles require desktop app");
          return true;
        }
        const doc = await listProfiles();
        if (!arg.trim()) {
          const names = doc.profiles
            .map((p) => `· ${p.name}${p.id === doc.active_id ? " ✓" : ""}`)
            .join("\n");
          alert(
            (ctx.locale === "zh" ? "当前档案：\n" : "Profiles:\n") +
              names +
              (ctx.locale === "zh" ? "\n\n用法：/profile 档案名称" : "\n\nUsage: /profile <name>"),
          );
          return true;
        }
        const q = arg.trim().toLowerCase();
        const hit =
          doc.profiles.find((p) => p.name.toLowerCase() === q) ??
          doc.profiles.find((p) => p.id.toLowerCase() === q);
        if (!hit) {
          alert(ctx.locale === "zh" ? `未找到档案：${arg}` : `Profile not found: ${arg}`);
          return true;
        }
        await activateProfile(hit.id);
        alert(ctx.locale === "zh" ? `已切换档案：${hit.name}` : `Activated profile: ${hit.name}`);
        return true;
      }
    }

    if (!ctx.activeId) {
      alert(ctx.locale === "zh" ? "请先新建或选择一个会话" : "Create or select a chat first");
      return true;
    }

    switch (cmd) {
      case "compact":
        await ctx.client.compactThread(ctx.activeId, arg || undefined);
        return true;

      case "fork": {
        const nt = await ctx.client.forkThread(ctx.activeId);
        await ctx.refresh();
        ctx.setActiveId(nt.id);
        return true;
      }

      case "clear": {
        const nt = await ctx.client.forkThread(ctx.activeId);
        await ctx.refresh();
        ctx.setActiveId(nt.id);
        return true;
      }

      case "review": {
        const target = arg.trim();
        const prompt = target
          ? `请对 ${target} 进行代码审查：检查潜在 bug、边界条件、安全与性能问题、可读性，并给出可执行的改进建议（按严重程度分级）。`
          : `请审查本工作区当前的代码改动：先用 git 查看变更，再检查潜在 bug、边界条件、安全与性能问题、可读性，并给出可执行的改进建议（按严重程度分级）。`;
        await ctx.client.startTurn(ctx.activeId, { prompt });
        return true;
      }

      case "export": {
        const detail = await ctx.client.getThread(ctx.activeId);
        const title = (detail.thread.title || "thread")
          .replace(/[^\w\u4e00-\u9fa5-]+/g, "_")
          .slice(0, 40);
        downloadJson(`deepseek-${title}-${ctx.activeId.slice(0, 8)}.json`, detail);
        return true;
      }

      case "model":
        if (!arg.trim()) {
          alert(
            (ctx.locale === "zh" ? "当前模型：" : "Current model: ") +
              (ctx.activeThread?.model ?? "?") +
              (ctx.locale === "zh" ? "\n用法：/model deepseek-v4-pro" : "\nUsage: /model deepseek-v4-pro"),
          );
          return true;
        }
        await ctx.onChangeThreadField({ model: arg.trim() });
        return true;

      case "models":
        alert((ctx.locale === "zh" ? "可用模型：\n" : "Models:\n") + ctx.models.join("\n"));
        return true;

      case "mode": {
        if (!arg.trim()) {
          alert(
            (ctx.locale === "zh" ? "当前模式：" : "Current mode: ") +
              (ctx.activeThread?.mode ?? "?") +
              (ctx.locale === "zh"
                ? "\n用法：/mode plan|agent|yolo"
                : "\nUsage: /mode plan|agent|yolo"),
          );
          return true;
        }
        const mode = parseModeArg(arg);
        if (!mode) {
          alert(ctx.locale === "zh" ? "无效模式，请用 plan、agent 或 yolo" : "Invalid mode");
          return true;
        }
        const patch: { mode: string; auto_approve?: boolean; trust_mode?: boolean } = { mode };
        if (mode === "yolo") {
          patch.auto_approve = true;
          patch.trust_mode = true;
        }
        await ctx.onChangeThreadField(patch);
        return true;
      }

      case "trust": {
        // list/add/remove 子命令打开信任目录面板；on/off 切换信任模式
        const sub = arg.trim().toLowerCase().split(/\s+/)[0];
        if (["list", "add", "remove", "rm", "ls"].includes(sub)) {
          ctx.openSettings("trust");
          return true;
        }
        if (!arg.trim()) {
          alert(
            (ctx.locale === "zh" ? "信任模式：" : "Trust mode: ") +
              (ctx.activeThread?.trust_mode ? "on" : "off") +
              (ctx.locale === "zh" ? "\n用法：/trust on|off" : "\nUsage: /trust on|off"),
          );
          return true;
        }
        const v = parseTrustArg(arg);
        if (v == null) {
          alert(ctx.locale === "zh" ? "用法：/trust on|off" : "Usage: /trust on|off");
          return true;
        }
        await ctx.onChangeThreadField({ trust_mode: v });
        return true;
      }

      case "rename": {
        const title =
          arg.trim() ||
          window.prompt(ctx.locale === "zh" ? "会话名称" : "Thread title", ctx.activeThread?.title || "")?.trim();
        if (!title) return true;
        await ctx.client.patchThread(ctx.activeId, { title });
        await ctx.refresh();
        return true;
      }

      case "cost":
      case "tokens":
        ctx.setShowUsage(true);
        return true;

      case "save":
        // GUI 的会话由后端持续持久化，无需手动保存；提示并引导导出
        alert(
          ctx.locale === "zh"
            ? "GUI 会话会自动保存。如需导出文件，请使用 /export。"
            : "GUI sessions auto-save. Use /export to write a file.",
        );
        return true;

      case "relay": {
        // 生成会话接力：指示模型写 .deepseek/handoff.md（对齐 TUI /relay）
        const focus = arg.trim();
        const instruction =
          `Create a compact session relay (接力) for a future thread and write it to ` +
          `\`.deepseek/handoff.md\`. Summarize the goal, key decisions, current state, ` +
          `open tasks, and any gotchas.` +
          (focus ? ` Focus especially on: ${focus}.` : "");
        await ctx.onSend(instruction);
        return true;
      }

      case "undo": {
        // 通过后端快照 API 撤销上一回合：还原到最近的 pre-turn 快照
        try {
          const res = await ctx.client.listSnapshots(ctx.activeId, 50);
          const preTurn = res.snapshots.find((s) => s.label.startsWith("pre-turn"));
          const target = preTurn ?? res.snapshots[0];
          if (!target) {
            alert(ctx.locale === "zh" ? "暂无可撤销的快照" : "No snapshot to undo");
            return true;
          }
          const r = await ctx.client.restoreSnapshot(ctx.activeId, target.id);
          ctx.afterRestore?.();
          alert(
            (ctx.locale === "zh" ? "已撤销到快照 " : "Reverted to ") +
              r.restored.slice(0, 8),
          );
        } catch (e) {
          alert((ctx.locale === "zh" ? "撤销失败：" : "Undo failed: ") + (e as Error).message);
        }
        return true;
      }

      case "restore": {
        const n = arg.trim();
        // 无参：打开快照浏览模态框
        if (!n) {
          ctx.setShowSnapshots(true);
          return true;
        }
        // /restore N：还原到第 N 新的快照（1 基，最新优先）
        const idx = Number(n);
        if (!Number.isInteger(idx) || idx < 1) {
          ctx.setShowSnapshots(true);
          return true;
        }
        try {
          const res = await ctx.client.listSnapshots(ctx.activeId, Math.max(idx, 50));
          const target = res.snapshots[idx - 1];
          if (!target) {
            alert(ctx.locale === "zh" ? `第 ${idx} 个快照不存在` : `Snapshot #${idx} not found`);
            return true;
          }
          const r = await ctx.client.restoreSnapshot(ctx.activeId, target.id);
          ctx.afterRestore?.();
          alert(
            (ctx.locale === "zh" ? "已还原到快照 " : "Restored to ") + r.restored.slice(0, 8),
          );
        } catch (e) {
          alert((ctx.locale === "zh" ? "还原失败：" : "Restore failed: ") + (e as Error).message);
        }
        return true;
      }

      case "agent": {
        // 解析 "/agent [N] <task>"：可选深度前缀 N(0-3) + 任务描述
        let depth = 1;
        let task = arg.trim();
        const m = task.match(/^(\d)\s+(.*)$/s);
        if (m) {
          const n = Number(m[1]);
          if (n >= 0 && n <= 3) {
            depth = n;
            task = m[2].trim();
          }
        }
        if (!task) {
          alert(
            ctx.locale === "zh"
              ? "用法：/agent [深度0-3] <任务描述>"
              : "Usage: /agent [depth 0-3] <task>",
          );
          return true;
        }
        // 指示模型开启持久子代理会话（对齐 TUI /agent 行为）
        const instruction =
          `Open a persistent sub-agent session for this task. Call \`agent_open\` with name ` +
          `\`slash_agent\`, \`prompt: ${JSON.stringify(task)}\`, and \`max_depth: ${depth}\`. ` +
          `Use \`agent_eval\` to wait for the next projection and \`handle_read\` on the returned ` +
          `transcript_handle if you need more detail. Verify any claimed side effects before reporting success.`;
        await ctx.onSend(instruction);
        return true;
      }

      case "retry": {
        if (!ctx.lastUserMessage) {
          alert(ctx.locale === "zh" ? "没有可重试的用户消息" : "No user message to retry");
          return true;
        }
        await ctx.onSend(ctx.lastUserMessage);
        return true;
      }

      case "attach": {
        let filePath = arg.trim();
        if (!filePath && isTauri()) {
          filePath = (await pickFile()) ?? "";
        }
        if (!filePath) {
          alert(ctx.locale === "zh" ? "用法：/attach 文件路径" : "Usage: /attach <path>");
          return true;
        }
        if (!ctx.rootPath) {
          alert(ctx.locale === "zh" ? "请先打开项目文件夹" : "Open a workspace folder first");
          return true;
        }
        const normRoot = ctx.rootPath.replace(/\\/g, "/").replace(/\/$/, "");
        const normFile = filePath.replace(/\\/g, "/");
        let rel: string;
        if (normFile.startsWith(normRoot + "/")) {
          rel = normFile.slice(normRoot.length + 1);
        } else if (normFile.startsWith(normRoot)) {
          rel = normFile.slice(normRoot.length).replace(/^\/+/, "");
        } else {
          rel = normFile;
        }
        ctx.insertAttachmentPath(rel);
        return true;
      }

      default:
        alert(
          ctx.locale === "zh" ? `未知命令：/${cmd}（输入 /help 查看）` : `Unknown: /${cmd} (see /help)`,
        );
        return true;
    }
  } catch (e) {
    alert((ctx.locale === "zh" ? "命令执行失败：" : "Command failed: ") + (e as Error).message);
    return true;
  }
}
