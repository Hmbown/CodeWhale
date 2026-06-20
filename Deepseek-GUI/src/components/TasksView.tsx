// 任务/自动化界面：创建并管理「后台自主任务」。
// 任务由后端 TaskManager 在独立工作区异步执行（队列/运行/完成/失败/取消），
// 与前台交互式会话相互独立。本组件负责：新建任务表单、任务列表轮询展示、取消任务。

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { TaskCounts, TaskStatus, TaskSummary } from "../api/types";
import { AutomationsView } from "./AutomationsView";

/** 子页签：后台任务 vs 定时自动化 */
type TasksTab = "tasks" | "automations";

/** 任务模式可选项（与会话模式一致） */
const TASK_MODES = ["agent", "plan", "yolo"];

/** 状态对应的中文与颜色 */
const STATUS_META: Record<TaskStatus, { label: string; color: string }> = {
  queued: { label: "排队中", color: "#c7a14a" },
  running: { label: "运行中", color: "#3b82f6" },
  completed: { label: "已完成", color: "#4fcc6b" },
  failed: { label: "失败", color: "#ef4444" },
  canceled: { label: "已取消", color: "#888" },
};

interface TasksViewProps {
  client: RuntimeClient;
  /** 默认工作区（左栏选中的项目文件夹），新建任务时作为默认 workspace */
  defaultWorkspace: string | null;
  onBack: () => void;
  /** 嵌入统一设置页：隐藏顶栏返回 */
  embedded?: boolean;
}

/**
 * 任务/自动化主界面。
 * @param client 运行时 API 客户端
 * @param defaultWorkspace 新建任务默认工作区
 * @param onBack 返回聊天界面回调
 */
export function TasksView({ client, defaultWorkspace, onBack, embedded }: TasksViewProps) {
  const [tab, setTab] = useState<TasksTab>("tasks");
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [counts, setCounts] = useState<TaskCounts | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 新建任务表单字段
  const [prompt, setPrompt] = useState("");
  const [mode, setMode] = useState("agent");
  const [workspace, setWorkspace] = useState(defaultWorkspace ?? "");
  const [allowShell, setAllowShell] = useState(false);
  const [autoApprove, setAutoApprove] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  /** 拉取任务列表（含计数） */
  const refresh = useCallback(async () => {
    try {
      const res = await client.listTasks(50);
      setTasks(res.tasks);
      setCounts(res.counts);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  // 首次加载 + 每 3 秒轮询，及时反映任务状态变化
  useEffect(() => {
    setLoading(true);
    refresh();
    const timer = setInterval(refresh, 3000);
    return () => clearInterval(timer);
  }, [refresh]);

  /** 提交新建任务 */
  const onCreate = useCallback(async () => {
    if (!prompt.trim()) {
      alert("请填写任务提示词。");
      return;
    }
    setSubmitting(true);
    try {
      await client.createTask({
        prompt: prompt.trim(),
        mode,
        workspace: workspace.trim() || undefined,
        allow_shell: allowShell,
        // yolo 模式默认自动批准；其余按开关
        auto_approve: autoApprove || mode === "yolo",
      });
      setPrompt("");
      await refresh();
    } catch (e) {
      alert(`创建任务失败：${(e as Error).message}`);
    } finally {
      setSubmitting(false);
    }
  }, [client, prompt, mode, workspace, allowShell, autoApprove, refresh]);

  /** 取消任务 */
  const onCancel = useCallback(
    async (id: string) => {
      try {
        await client.cancelTask(id);
        await refresh();
      } catch (e) {
        alert(`取消失败：${(e as Error).message}`);
      }
    },
    [client, refresh],
  );

  return (
    <div className={`tasks-view${embedded ? " tasks-view-embedded" : ""}`}>
      {!embedded && (
      <div className="tasks-head">
        <span className="pane-title">任务 / 自动化</span>
        <button className="btn-mini" onClick={onBack} title="返回聊天">
          ← 返回
        </button>
      </div>
      )}

      {/* 子页签：后台任务 | 定时自动化 */}
      <div className="view-tabs">
        <button
          type="button"
          className={`view-tab${tab === "tasks" ? " active" : ""}`}
          onClick={() => setTab("tasks")}
        >
          后台任务
        </button>
        <button
          type="button"
          className={`view-tab${tab === "automations" ? " active" : ""}`}
          onClick={() => setTab("automations")}
        >
          定时自动化
        </button>
      </div>

      {tab === "automations" ? (
        <AutomationsView client={client} onBack={() => setTab("tasks")} embedded />
      ) : (
        <>
      <div className="task-form">
        <textarea
          className="task-prompt"
          placeholder="描述要后台自主完成的任务，例如：在当前项目里实现登录页并补充单测…"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          rows={3}
        />
        <input
          className="task-input"
          placeholder="工作区路径（留空使用后端默认）"
          value={workspace}
          onChange={(e) => setWorkspace(e.target.value)}
        />
        <div className="task-form-row">
          <select
            className="mini-select"
            value={mode}
            onChange={(e) => setMode(e.target.value)}
          >
            {TASK_MODES.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
          <label className="task-check">
            <input
              type="checkbox"
              checked={allowShell}
              onChange={(e) => setAllowShell(e.target.checked)}
            />
            允许执行命令
          </label>
          <label className="task-check">
            <input
              type="checkbox"
              checked={autoApprove}
              onChange={(e) => setAutoApprove(e.target.checked)}
            />
            自动批准
          </label>
          <button
            className="btn primary"
            onClick={onCreate}
            disabled={submitting}
          >
            {submitting ? "提交中…" : "创建任务"}
          </button>
        </div>
      </div>

      {/* 状态计数条 */}
      {counts && (
        <div className="task-counts">
          {(Object.keys(STATUS_META) as TaskStatus[]).map((s) => (
            <span key={s} className="task-count">
              <i style={{ background: STATUS_META[s].color }} />
              {STATUS_META[s].label} {counts[s as keyof TaskCounts]}
            </span>
          ))}
        </div>
      )}

      {/* 任务列表 */}
      <div className="task-list">
        {error && <div className="banner banner-warn">加载失败：{error}</div>}
        {!error && loading && tasks.length === 0 && (
          <div className="pane-placeholder">加载中…</div>
        )}
        {!error && !loading && tasks.length === 0 && (
          <div className="pane-placeholder">还没有任务，先在上方创建一个。</div>
        )}
        {tasks.map((t) => {
          const meta = STATUS_META[t.status];
          const active = t.status === "queued" || t.status === "running";
          return (
            <div key={t.id} className="task-card">
              <div className="task-card-top">
                <span
                  className="task-badge"
                  style={{ background: meta.color }}
                >
                  {meta.label}
                </span>
                <span className="task-model">{t.model}</span>
                <span className="task-mode">{t.mode}</span>
                {t.duration_ms != null && (
                  <span className="task-dur">
                    {(t.duration_ms / 1000).toFixed(1)}s
                  </span>
                )}
                {active && (
                  <button
                    className="btn-mini task-cancel"
                    onClick={() => onCancel(t.id)}
                    title="取消任务"
                  >
                    取消
                  </button>
                )}
              </div>
              <div className="task-prompt-text">{t.prompt_summary}</div>
              {t.error && <div className="task-error">错误：{t.error}</div>}
              <div className="task-meta">
                <span>{new Date(t.created_at).toLocaleString()}</span>
                <span className="task-id">{t.id.slice(0, 8)}</span>
              </div>
            </div>
          );
        })}
      </div>
        </>
      )}
    </div>
  );
}
