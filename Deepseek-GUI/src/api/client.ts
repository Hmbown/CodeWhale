// 运行时 API 的 REST 客户端封装
// 统一处理 base URL、可选鉴权令牌、错误解析

import type {
  ApprovalDecision,
  CreateThreadRequest,
  RuntimeInfo,
  StartTurnRequest,
  ThreadDetail,
  ThreadRecord,
  ThreadSummary,
  TurnRecord,
  UsageAggregation,
  TasksResponse,
  JobsResponse,
  ShellJobDetail,
  SubagentsResponse,
  SubAgentResult,
  RlmSessionsResponse,
  RlmSessionSummary,
  SnapshotsResponse,
  RestoreSnapshotResponse,
  NewTaskRequest,
  SkillsResponse,
  McpServersResponse,
  SessionsResponse,
  ResumeSessionResponse,
  WorkspaceStatus,
  McpToolsResponse,
  AutomationRecord,
  AutomationRunRecord,
  CreateAutomationRequest,
} from "./types";

/** 默认后端地址，与 deepseek serve --http 的默认监听一致 */
export const DEFAULT_BASE_URL = "http://127.0.0.1:7878";

/** 客户端配置 */
export interface ClientConfig {
  baseUrl: string;
  /** 可选鉴权令牌（对应 --auth-token / DEEPSEEK_RUNTIME_TOKEN） */
  token?: string;
}

/** 运行时 API 客户端 */
export class RuntimeClient {
  constructor(private cfg: ClientConfig) {}

  /** 组装请求头，按需附加鉴权令牌 */
  private headers(extra?: Record<string, string>): Record<string, string> {
    const h: Record<string, string> = { "Content-Type": "application/json", ...extra };
    if (this.cfg.token) {
      // 后端同时支持 Authorization 与自定义头，这里用 Bearer
      h["Authorization"] = `Bearer ${this.cfg.token}`;
    }
    return h;
  }

  /** 统一的请求执行与错误处理 */
  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await fetch(`${this.cfg.baseUrl}${path}`, {
      ...init,
      headers: this.headers(init?.headers as Record<string, string>),
    });
    if (!res.ok) {
      let detail = "";
      try {
        detail = await res.text();
      } catch {
        // 忽略读取失败
      }
      throw new Error(`HTTP ${res.status} ${res.statusText}${detail ? `: ${detail}` : ""}`);
    }
    // 部分接口可能无返回体
    const text = await res.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }

  /** 健康检查 */
  async health(): Promise<boolean> {
    try {
      const res = await fetch(`${this.cfg.baseUrl}/health`);
      return res.ok;
    } catch {
      return false;
    }
  }

  /** 运行时信息 */
  runtimeInfo(): Promise<RuntimeInfo> {
    return this.request<RuntimeInfo>("/v1/runtime/info");
  }

  /** 列出线程 */
  listThreads(includeArchived = false): Promise<ThreadRecord[]> {
    return this.request<ThreadRecord[]>(
      `/v1/threads?limit=100&include_archived=${includeArchived}`,
    );
  }

  /** 搜索线程摘要（标题/预览模糊匹配） */
  searchThreads(search?: string, limit = 30): Promise<ThreadSummary[]> {
    const q = new URLSearchParams();
    q.set("limit", String(limit));
    q.set("include_archived", "true");
    if (search) q.set("search", search);
    return this.request<ThreadSummary[]>(`/v1/threads/summary?${q.toString()}`);
  }

  /** 新建线程 */
  createThread(body: CreateThreadRequest): Promise<ThreadRecord> {
    return this.request<ThreadRecord>("/v1/threads", {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  /** 获取线程详情（含历史 turns 与 items） */
  getThread(id: string): Promise<ThreadDetail> {
    return this.request<ThreadDetail>(`/v1/threads/${id}`);
  }

  /** 预加载线程引擎（转向/续聊前确保后端已加载会话） */
  resumeThread(id: string): Promise<ThreadRecord> {
    return this.request<ThreadRecord>(`/v1/threads/${id}/resume`, {
      method: "POST",
    });
  }

  /** 归档/取消归档等部分更新 */
  patchThread(id: string, patch: Record<string, unknown>): Promise<ThreadRecord> {
    return this.request<ThreadRecord>(`/v1/threads/${id}`, {
      method: "PATCH",
      body: JSON.stringify(patch),
    });
  }

  /** 发送消息（发起一个回合） */
  startTurn(threadId: string, body: StartTurnRequest): Promise<{ thread: ThreadRecord; turn: TurnRecord }> {
    return this.request(`/v1/threads/${threadId}/turns`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  /** 转向（向进行中的回合追加指令） */
  steerTurn(threadId: string, turnId: string, prompt: string): Promise<TurnRecord> {
    return this.request(`/v1/threads/${threadId}/turns/${turnId}/steer`, {
      method: "POST",
      body: JSON.stringify({ prompt }),
    });
  }

  /** 打断回合 */
  interruptTurn(threadId: string, turnId: string): Promise<TurnRecord> {
    return this.request(`/v1/threads/${threadId}/turns/${turnId}/interrupt`, {
      method: "POST",
    });
  }

  /** 获取用量统计（默认按线程聚合，便于汇总会话总量） */
  getUsage(): Promise<UsageAggregation> {
    return this.request<UsageAggregation>("/v1/usage?group_by=thread");
  }

  /** 压缩上下文（/compact）：发起一次压缩回合 */
  compactThread(id: string, reason?: string): Promise<unknown> {
    return this.request(`/v1/threads/${id}/compact`, {
      method: "POST",
      body: JSON.stringify({ reason: reason ?? null }),
    });
  }

  /** 复刻会话（/fork）：基于当前会话创建副本，返回新会话 */
  forkThread(id: string): Promise<ThreadRecord> {
    return this.request<ThreadRecord>(`/v1/threads/${id}/fork`, {
      method: "POST",
    });
  }

  /** 列出后台任务（含各状态计数） */
  listTasks(limit = 50): Promise<TasksResponse> {
    return this.request<TasksResponse>(`/v1/tasks?limit=${limit}`);
  }

  /** 新建后台任务（自主运行） */
  createTask(req: NewTaskRequest): Promise<unknown> {
    return this.request("/v1/tasks", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  /** 取消队列中或运行中的任务 */
  cancelTask(id: string): Promise<unknown> {
    return this.request(`/v1/tasks/${id}/cancel`, { method: "POST" });
  }

  /** 列出后台 Shell 作业（TUI /jobs） */
  listJobs(opts?: { status?: string; limit?: number }): Promise<JobsResponse> {
    const q = new URLSearchParams();
    if (opts?.status) q.set("status", opts.status);
    if (opts?.limit != null) q.set("limit", String(opts.limit));
    const qs = q.toString();
    return this.request<JobsResponse>(`/v1/jobs${qs ? `?${qs}` : ""}`);
  }

  /** 获取 Shell 作业详情 */
  getJob(id: string): Promise<ShellJobDetail> {
    return this.request<ShellJobDetail>(`/v1/jobs/${encodeURIComponent(id)}`);
  }

  /** 取消 Shell 作业 */
  cancelJob(id: string): Promise<unknown> {
    return this.request(`/v1/jobs/${encodeURIComponent(id)}/cancel`, { method: "POST" });
  }

  /** 向 Shell 作业写入 stdin */
  writeJobStdin(id: string, input: string, close = false): Promise<void> {
    return this.request(`/v1/jobs/${encodeURIComponent(id)}/stdin`, {
      method: "POST",
      body: JSON.stringify({ input, close }),
    });
  }

  /** 列出 Subagent 状态 */
  listSubagents(opts?: { includeArchived?: boolean; limit?: number }): Promise<SubagentsResponse> {
    const q = new URLSearchParams();
    if (opts?.includeArchived) q.set("include_archived", "true");
    if (opts?.limit != null) q.set("limit", String(opts.limit));
    const qs = q.toString();
    return this.request<SubagentsResponse>(`/v1/subagents${qs ? `?${qs}` : ""}`);
  }

  /** 获取单个 Subagent */
  getSubagent(agentId: string): Promise<SubAgentResult> {
    return this.request<SubAgentResult>(`/v1/subagents/${encodeURIComponent(agentId)}`);
  }

  /** 取消运行中的 Subagent */
  cancelSubagent(agentId: string): Promise<SubAgentResult> {
    return this.request<SubAgentResult>(
      `/v1/subagents/${encodeURIComponent(agentId)}/cancel`,
      { method: "POST" },
    );
  }

  /** 列出 RLM 会话 */
  listRlmSessions(): Promise<RlmSessionsResponse> {
    return this.request<RlmSessionsResponse>("/v1/rlm/sessions");
  }

  /** 获取单个 RLM 会话 */
  getRlmSession(name: string): Promise<RlmSessionSummary> {
    return this.request<RlmSessionSummary>(`/v1/rlm/sessions/${encodeURIComponent(name)}`);
  }

  /** 列出会话工作区的快照（最新优先） */
  listSnapshots(threadId: string, limit = 50): Promise<SnapshotsResponse> {
    return this.request<SnapshotsResponse>(
      `/v1/threads/${encodeURIComponent(threadId)}/snapshots?limit=${limit}`,
    );
  }

  /** 还原工作区到指定快照（缺省 snapshotId 时还原到最近一条） */
  restoreSnapshot(threadId: string, snapshotId?: string): Promise<RestoreSnapshotResponse> {
    return this.request<RestoreSnapshotResponse>(
      `/v1/threads/${encodeURIComponent(threadId)}/snapshots/restore`,
      {
        method: "POST",
        body: JSON.stringify(snapshotId ? { snapshot_id: snapshotId } : {}),
      },
    );
  }

  /** 列出技能（含启用状态、技能目录与告警） */
  listSkills(): Promise<SkillsResponse> {
    return this.request<SkillsResponse>("/v1/skills");
  }

  /** 启用/禁用某个技能 */
  setSkillEnabled(name: string, enabled: boolean): Promise<unknown> {
    return this.request(`/v1/skills/${encodeURIComponent(name)}`, {
      method: "POST",
      body: JSON.stringify({ enabled }),
    });
  }

  /** 列出已配置的 MCP 服务器及其连接/工具状态 */
  listMcpServers(): Promise<McpServersResponse> {
    return this.request<McpServersResponse>("/v1/apps/mcp/servers");
  }

  /** 列出历史会话（可搜索） */
  listSessions(search?: string, limit = 50): Promise<SessionsResponse> {
    const q = new URLSearchParams();
    q.set("limit", String(limit));
    if (search) q.set("search", search);
    return this.request<SessionsResponse>(`/v1/sessions?${q.toString()}`);
  }

  /** 将历史会话恢复为一个新线程 */
  resumeSession(id: string, body?: { model?: string; mode?: string }): Promise<ResumeSessionResponse> {
    return this.request<ResumeSessionResponse>(`/v1/sessions/${id}/resume-thread`, {
      method: "POST",
      body: JSON.stringify(body ?? {}),
    });
  }

  /** 删除历史会话 */
  deleteSession(id: string): Promise<unknown> {
    return this.request(`/v1/sessions/${id}`, { method: "DELETE" });
  }

  /** 回应审批请求 */
  decideApproval(approvalId: string, decision: ApprovalDecision): Promise<unknown> {
    return this.request(`/v1/approvals/${approvalId}`, {
      method: "POST",
      body: JSON.stringify(decision),
    });
  }

  /** 获取 Git 工作区状态 */
  getWorkspaceStatus(): Promise<WorkspaceStatus> {
    return this.request<WorkspaceStatus>("/v1/workspace/status");
  }

  /** 列出 MCP 工具（可选按 server 过滤） */
  listMcpTools(server?: string): Promise<McpToolsResponse> {
    const q = server ? `?server=${encodeURIComponent(server)}` : "";
    return this.request<McpToolsResponse>(`/v1/apps/mcp/tools${q}`);
  }

  /** 列出定时自动化 */
  listAutomations(): Promise<AutomationRecord[]> {
    return this.request<AutomationRecord[]>("/v1/automations");
  }

  /** 新建定时自动化 */
  createAutomation(body: CreateAutomationRequest): Promise<AutomationRecord> {
    return this.request<AutomationRecord>("/v1/automations", {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  /** 删除自动化 */
  deleteAutomation(id: string): Promise<unknown> {
    return this.request(`/v1/automations/${id}`, { method: "DELETE" });
  }

  /** 立即运行自动化 */
  runAutomation(id: string): Promise<unknown> {
    return this.request(`/v1/automations/${id}/run`, { method: "POST" });
  }

  /** 暂停自动化 */
  pauseAutomation(id: string): Promise<unknown> {
    return this.request(`/v1/automations/${id}/pause`, { method: "POST" });
  }

  /** 恢复自动化 */
  resumeAutomation(id: string): Promise<unknown> {
    return this.request(`/v1/automations/${id}/resume`, { method: "POST" });
  }

  /** 列出自动化运行记录 */
  listAutomationRuns(id: string, limit = 10): Promise<AutomationRunRecord[]> {
    return this.request<AutomationRunRecord[]>(`/v1/automations/${id}/runs?limit=${limit}`);
  }
}

/**
 * 轮询等待后端就绪（切换工作目录重启后端后使用）。
 * @returns 是否在超时前检测到健康
 */
export async function waitForBackend(
  client: RuntimeClient,
  opts?: { timeoutMs?: number; intervalMs?: number },
): Promise<boolean> {
  const timeoutMs = opts?.timeoutMs ?? 30000;
  const intervalMs = opts?.intervalMs ?? 400;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await client.health()) return true;
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  return false;
}
