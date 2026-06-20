// 运行时 API 数据类型定义
// 字段依据 crates/tui/src/runtime_threads.rs 与 runtime_api.rs 的真实结构体

/** 线程内 item 的类型，决定前端渲染样式 */
export type TurnItemKind =
  | "user_message"
  | "agent_message"
  | "agent_reasoning"
  | "tool_call"
  | "file_change"
  | "command_execution"
  | "context_compaction"
  | "status"
  | "error";

/** 会话模式 */
export type ThreadMode = "plan" | "agent" | "yolo";

/** 线程记录（与后端 ThreadRecord 对齐，仅保留前端关心字段） */
export interface ThreadRecord {
  id: string;
  created_at: string;
  updated_at: string;
  model: string;
  workspace: string;
  mode: string;
  archived: boolean;
  system_prompt?: string | null;
  latest_turn_id?: string | null;
  title?: string | null;
  /** 是否允许 Shell 工具 */
  allow_shell?: boolean;
  /** 信任模式（放宽沙箱） */
  trust_mode?: boolean;
  /** 自动批准工具调用（YOLO 行为） */
  auto_approve?: boolean;
}

/** 回合状态 */
export type TurnStatus =
  | "queued"
  | "in_progress"
  | "completed"
  | "failed"
  | "interrupted"
  | "canceled";

/** 回合记录 */
export interface TurnRecord {
  id: string;
  thread_id: string;
  status: TurnStatus;
  created_at?: string;
  duration_ms?: number | null;
  error_summary?: string | null;
}

/** 线程详情（GET /v1/threads/{id} 返回） */
export interface ThreadDetail {
  thread: ThreadRecord;
  turns: TurnRecord[];
  items: TurnItemRecord[];
  latest_seq: number;
}

/** 单个 turn item 记录 */
export interface TurnItemRecord {
  id: string;
  turn_id: string;
  kind: TurnItemKind;
  status?: string;
  metadata?: Record<string, unknown>;
}

/** 新建线程请求体（全部字段可选） */
export interface CreateThreadRequest {
  model?: string;
  workspace?: string;
  mode?: string;
  allow_shell?: boolean;
  trust_mode?: boolean;
  auto_approve?: boolean;
  archived?: boolean;
  system_prompt?: string;
  task_id?: string;
}

/** 发起回合请求体（prompt 必填） */
export interface StartTurnRequest {
  prompt: string;
  input_summary?: string;
  model?: string;
  mode?: string;
  allow_shell?: boolean;
  trust_mode?: boolean;
  auto_approve?: boolean;
}

/** 审批决定 */
export interface ApprovalDecision {
  // 后端约定值：allow=批准，deny=拒绝
  decision: "allow" | "deny";
  remember?: boolean;
}

/** SSE 事件信封（与 runtime_event_payload 的输出对齐） */
export interface RuntimeEvent {
  seq: number;
  timestamp: string;
  thread_id: string;
  turn_id?: string | null;
  item_id?: string | null;
  event: string;
  payload: Record<string, unknown>;
}

/** 运行时信息 */
export interface RuntimeInfo {
  bind_host: string;
  port: number;
  auth_required: boolean;
  version: string;
}

/** 用量合计（与后端 UsageTotals 对齐） */
export interface UsageTotals {
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  reasoning_tokens: number;
  cost_usd: number;
  turns: number;
}

/** 用量分桶（group_by=thread 等） */
export interface UsageBucket {
  key: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens?: number;
  reasoning_tokens?: number;
  cost_usd: number;
  turns: number;
}

/** 用量聚合响应 */
export interface UsageAggregation {
  group_by: string;
  totals: UsageTotals;
  buckets: UsageBucket[];
}

/** 后台任务状态（与后端 TaskStatus 的 snake_case 对齐） */
export type TaskStatus = "queued" | "running" | "completed" | "failed" | "canceled";

/** 任务摘要（列表项，对应后端 TaskSummary） */
export interface TaskSummary {
  id: string;
  status: TaskStatus;
  prompt_summary: string;
  model: string;
  mode: string;
  created_at: string;
  started_at?: string | null;
  ended_at?: string | null;
  duration_ms?: number | null;
  error?: string | null;
  thread_id?: string | null;
  turn_id?: string | null;
}

/** 各状态任务计数（对应后端 TaskCounts） */
export interface TaskCounts {
  queued: number;
  running: number;
  completed: number;
  failed: number;
  canceled: number;
}

/** 任务列表响应（对应后端 TasksResponse） */
export interface TasksResponse {
  tasks: TaskSummary[];
  counts: TaskCounts;
}

/** Shell 作业状态（对应后端 ShellStatus） */
export type ShellJobStatus = "Running" | "Completed" | "Failed" | "Killed" | "TimedOut";

/** Shell 作业快照（GET /v1/jobs） */
export interface ShellJobSnapshot {
  id: string;
  job_id: string;
  command: string;
  cwd: string;
  status: ShellJobStatus;
  exit_code: number | null;
  elapsed_ms: number;
  stdout_tail: string;
  stderr_tail: string;
  stdout_len: number;
  stderr_len: number;
  stdin_available: boolean;
  stale: boolean;
  linked_task_id: string | null;
}

/** Shell 作业详情 */
export interface ShellJobDetail {
  snapshot: ShellJobSnapshot;
  stdout: string;
  stderr: string;
}

/** Jobs 列表响应 */
export interface JobsResponse {
  jobs: ShellJobSnapshot[];
  running_count: number;
}

/** Subagent 状态 */
export type SubAgentStatus =
  | "Running"
  | { Completed: null }
  | { Interrupted: string }
  | { Failed: string }
  | "Cancelled";

/** Subagent 快照（GET /v1/subagents） */
export interface SubAgentResult {
  name: string;
  agent_id: string;
  context_mode: string;
  fork_context: boolean;
  agent_type: string;
  assignment: { objective: string; role: string };
  model: string;
  nickname?: string | null;
  status: SubAgentStatus | string;
  result: string | null;
  steps_taken: number;
  duration_ms: number;
  from_prior_session?: boolean;
}

/** Subagents 列表响应 */
export interface SubagentsResponse {
  agents: SubAgentResult[];
  running_count: number;
}

/** RLM context 元数据 */
export interface RlmContextMeta {
  length: number;
  type: string;
  preview_500: string;
  sha256: string;
}

/** RLM 会话摘要 */
export interface RlmSessionSummary {
  name: string;
  id: string;
  context_meta: RlmContextMeta;
  config: {
    output_feedback: string;
    sub_query_timeout_secs: number;
    sub_rlm_max_depth: number;
    share_session: boolean;
  };
  rpc_count: number;
  total_duration_ms: number;
  peak_var_count: number;
  final_count: number;
  context_path: string;
  is_open: boolean;
  created_ms_ago: number;
  last_used_ms_ago: number;
}

/** RLM 列表响应 */
export interface RlmSessionsResponse {
  sessions: RlmSessionSummary[];
  open_count: number;
}

/** 单条工作区快照（pre/post-turn 安全网，对应后端 SnapshotEntry） */
export interface SnapshotEntry {
  /** 快照 id（side-repo 提交 SHA） */
  id: string;
  /** 标签，如 pre-turn:3 / post-turn:3 / pre-restore:... */
  label: string;
  /** 作者时间戳（Unix 秒） */
  timestamp: number;
}

/** 快照列表响应（对应后端 SnapshotsResponse） */
export interface SnapshotsResponse {
  /** 该会话工作区路径 */
  workspace: string;
  /** 快照条目（最新优先） */
  snapshots: SnapshotEntry[];
}

/** 还原响应（对应后端 RestoreSnapshotResponse） */
export interface RestoreSnapshotResponse {
  /** 实际还原到的快照 id */
  restored: string;
  /** 还原前自动创建的安全快照 id（便于反悔） */
  safety_snapshot: string | null;
}

/** 新建任务请求（对应后端 NewTaskRequest，可选项缺省时后端用安全默认值） */
export interface NewTaskRequest {
  prompt: string;
  model?: string;
  workspace?: string;
  mode?: string;
  allow_shell?: boolean;
  trust_mode?: boolean;
  auto_approve?: boolean;
}

/** 单个技能条目（对应后端 SkillEntry） */
export interface SkillEntry {
  name: string;
  description: string;
  path: string;
  enabled: boolean;
}

/** 技能列表响应（对应后端 SkillsResponse） */
export interface SkillsResponse {
  directory: string;
  warnings: string[];
  skills: SkillEntry[];
}

/** MCP 服务器条目（对应后端 McpServerEntry） */
export interface McpServerEntry {
  name: string;
  enabled: boolean;
  required: boolean;
  command?: string | null;
  url?: string | null;
  connected: boolean;
  enabled_tools: string[];
  disabled_tools: string[];
}

/** MCP 服务器列表响应（对应后端 McpServersResponse） */
export interface McpServersResponse {
  servers: McpServerEntry[];
}

/** 历史会话元数据（对应后端 SessionMetadata，保留前端关心字段） */
export interface SessionMetadata {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
  total_tokens: number;
  model: string;
  workspace: string;
  mode?: string | null;
}

/** 会话列表响应（对应后端 SessionsResponse） */
export interface SessionsResponse {
  sessions: SessionMetadata[];
}

/** 线程搜索摘要（GET /v1/threads/summary） */
export interface ThreadSummary {
  id: string;
  title: string;
  preview: string;
  model: string;
  mode: string;
  archived: boolean;
  updated_at: string;
  latest_turn_id?: string | null;
  latest_turn_status?: string | null;
}

/** 恢复会话为线程的响应（对应后端 ResumeSessionResponse） */
export interface ResumeSessionResponse {
  thread_id: string;
  session_id: string;
  message_count: number;
  summary: string;
}

/** Git 工作区状态（GET /v1/workspace/status） */
export interface WorkspaceStatus {
  workspace: string;
  git_repo: boolean;
  branch?: string | null;
  staged: number;
  unstaged: number;
  untracked: number;
  ahead?: number | null;
  behind?: number | null;
}

/** MCP 工具条目（GET /v1/apps/mcp/tools） */
export interface McpToolEntry {
  server: string;
  name: string;
  description?: string;
}

/** MCP 工具列表响应 */
export interface McpToolsResponse {
  tools: McpToolEntry[];
}

/** 自动化状态 */
export type AutomationStatus = "active" | "paused";

/** 自动化记录（对应后端 AutomationRecord） */
export interface AutomationRecord {
  id: string;
  name: string;
  prompt: string;
  rrule: string;
  cwds?: string[];
  status: AutomationStatus;
  created_at: string;
  updated_at: string;
  next_run_at?: string | null;
  last_run_at?: string | null;
}

/** 自动化运行记录 */
export interface AutomationRunRecord {
  id: string;
  automation_id: string;
  scheduled_for: string;
  status: string;
  created_at: string;
  started_at?: string | null;
  ended_at?: string | null;
  error?: string | null;
}

/** 新建自动化请求 */
export interface CreateAutomationRequest {
  name: string;
  prompt: string;
  rrule: string;
  cwds?: string[];
  status?: AutomationStatus;
}
