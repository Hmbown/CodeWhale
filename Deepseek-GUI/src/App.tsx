// 应用主组件：连接后端、管理会话列表与当前会话、消息收发与审批

import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { RuntimeClient, DEFAULT_BASE_URL, waitForBackend, type ClientConfig } from "./api/client";
import type { ThreadRecord, UsageTotals, WorkspaceStatus } from "./api/types";
import { useConversation } from "./state/useConversation";
import { MessageItem } from "./components/MessageItem";
import { Composer } from "./components/Composer";
import { QueueBar } from "./components/QueueBar";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { SettingsView, type SettingsTab } from "./components/SettingsView";
import { AgentHistoryPanel } from "./components/AgentHistoryPanel";
import { DiffModal } from "./components/DiffModal";
import { SnapshotsModal } from "./components/SnapshotsModal";
import { TerminalPanel } from "./components/TerminalPanel";
import { UsageModal } from "./components/UsageModal";
import { CommandPalette, type PaletteCommand } from "./components/CommandPalette";
import { QuickOpen } from "./components/QuickOpen";
import { ThreadSearch } from "./components/ThreadSearch";
import { NoticeList } from "./components/NoticeList";
import { RuleComplianceBanner } from "./components/RuleComplianceBanner";
import { FileTree } from "./components/FileTree";
import { EditorPanel } from "./components/EditorPanel";
import { TitleMenuBar } from "./components/TitleMenuBar";
import { StatusZoom } from "./components/StatusZoom";
import { getRuntimeToken, getConfig, isTauri, pickFolder, restartBackend, saveConfig, setWorkspace } from "./api/tauri";
import { executeSlashCommand } from "./utils/executeSlashCommand";
import { cycleReasoningEffort, type ReasoningEffort } from "./utils/reasoningEffort";
import { useEditorTabs } from "./hooks/useEditorTabs";
import { useUiZoom } from "./hooks/useUiZoom";
import { useResizablePanels } from "./hooks/useResizablePanels";
import { useTranscriptAutoScroll } from "./hooks/useTranscriptAutoScroll";
import { useRuleCompliance } from "./hooks/useRuleCompliance";
import { resolveWorkspacePath, workspacePathsEqual } from "./utils/workspacePaths";
import {
  filterThreadsForWorkspace,
  pickThreadForWorkspace,
  saveLastThreadForWorkspace,
} from "./utils/workspaceSessions";
import { loadLocale, saveLocale, t, type Locale } from "./i18n";
import { deriveThreadTitleFromMessage, formatThreadTabTitle, isUntitledThread } from "./utils/threadTitle";

/** 可选模型与模式 */
const MODELS = ["deepseek-v4-pro", "deepseek-v4-flash", "auto"];
const MODES = ["plan", "agent", "yolo"];

/** 从 localStorage 读取已保存的连接配置 */
function loadCfg(): ClientConfig {
  const baseUrl = localStorage.getItem("ds_base_url") || DEFAULT_BASE_URL;
  // 默认携带本地开发 token（与 Tauri 壳启动后端时设置的值一致）
  const token = localStorage.getItem("ds_token") || "dev-local-token";
  return { baseUrl, token };
}

export function App() {
  // 后端连接配置（可在顶部连接栏修改 baseUrl/token，持久化到 localStorage）
  const [cfg, setCfg] = useState<ClientConfig>(loadCfg);
  const client = useMemo(() => new RuntimeClient(cfg), [cfg]);

  // 连接栏的临时输入值
  const [formUrl, setFormUrl] = useState(cfg.baseUrl);
  const [formToken, setFormToken] = useState(cfg.token ?? "");

  /** 保存连接配置并触发重连 */
  const applyConn = () => {
    const next: ClientConfig = {
      baseUrl: formUrl.trim() || DEFAULT_BASE_URL,
      token: formToken.trim() || undefined,
    };
    localStorage.setItem("ds_base_url", next.baseUrl);
    if (next.token) localStorage.setItem("ds_token", next.token);
    else localStorage.removeItem("ds_token");
    setCfg(next);
  };

  const [backendUp, setBackendUp] = useState<boolean | null>(null);
  const [threads, setThreads] = useState<ThreadRecord[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  /** 设置页当前分类（Cursor 风格侧栏） */
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("models");
  // 历史会话浏览器开关
  const [showSessions, setShowSessions] = useState(false);
  // 全量变更（/diff）模态开关
  const [showDiff, setShowDiff] = useState(false);
  // 快照还原模态框（/restore）
  const [showSnapshots, setShowSnapshots] = useState(false);
  // 底部集成终端面板（VS Code 风格）：是否显示
  const [showTerminal, setShowTerminal] = useState(false);
  // 终端是否已被打开过（首次打开后保持挂载，避免反复 spawn PTY）
  const [terminalEverOpened, setTerminalEverOpened] = useState(false);
  // 终端面板高度（px），可拖动调整
  const [termHeight, setTermHeight] = useState(260);
  // 切换底部终端显示；首次打开时标记已挂载
  const toggleTerminal = useCallback(() => {
    setShowTerminal((v) => {
      const next = !v;
      if (next) setTerminalEverOpened(true);
      return next;
    });
  }, []);
  // 拖动终端面板顶边调整高度
  const onTermResizeStart = useCallback((e: ReactMouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = termHeight;
    const onMove = (ev: MouseEvent) => {
      const delta = startY - ev.clientY;
      // 限制在 120~视口高度-160 之间，避免压垮其他区域
      const max = window.innerHeight - 160;
      setTermHeight(Math.max(120, Math.min(max, startH + delta)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, [termHeight]);
  /** Token 用量弹窗（/cost / /tokens） */
  const [showUsage, setShowUsage] = useState(false);
  /** 推理强度（config.toml，Shift+Tab 切换） */
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("medium");
  /** Composer 插入 @ 路径（/attach 命令回调） */
  const composerInsertRef = useRef<((relPath: string) => void) | null>(null);
  // 命令面板（Ctrl+K）开关
  const [showPalette, setShowPalette] = useState(false);
  // 快速打开文件（Ctrl+P）
  const [showQuickOpen, setShowQuickOpen] = useState(false);
  // 搜索会话（Ctrl+Shift+P）
  const [showThreadSearch, setShowThreadSearch] = useState(false);
  /** 界面语言 */
  const [locale, setLocale] = useState<Locale>(loadLocale);
  /** 会话 system_prompt 编辑草稿 */
  const [systemPromptDraft, setSystemPromptDraft] = useState("");
  const [showSystemPrompt, setShowSystemPrompt] = useState(false);
  /** Git 工作区状态（分支、变更数） */
  const [wsStatus, setWsStatus] = useState<WorkspaceStatus | null>(null);
  /** 左侧资源管理器是否展开（仿 Cursor 活动栏切换） */
  const [sidebarOpen, setSidebarOpen] = useState(true);
  // 右侧聊天面板是否展开（仿 Cursor 两边收起）
  const [chatOpen, setChatOpen] = useState(true);
  // 是否在标签栏显示已归档会话
  const [showArchived, setShowArchived] = useState(false);
  // 命令菜单开关
  const [showCmd, setShowCmd] = useState(false);
  // 新建会话时使用的默认模型/模式
  const [newModel, setNewModel] = useState(MODELS[0]);
  const [newMode, setNewMode] = useState("agent");
  // 会话用量统计
  const [usage, setUsage] = useState<UsageTotals | null>(null);
  // 三栏 IDE：项目根目录与当前打开的文件
  const [rootPath, setRootPath] = useState<string | null>(
    () => localStorage.getItem("ds_root") || null,
  );
  // 多标签编辑器状态
  const editor = useEditorTabs();
  // 可拖拽分栏宽度（传入 sidebarOpen 以便 clamp 计算）
  const panels = useResizablePanels(sidebarOpen);
  /** 全局 UI 缩放（整窗，仿 Cursor） */
  const uiZoom = useUiZoom();
  // 文件树刷新令牌：自增即触发文件树重新读盘
  const [treeTick, setTreeTick] = useState(0);
  // Composer 消息队列：回合进行中排队，结束后自动按序发送（对齐 TUI /queue）
  const [queued, setQueued] = useState<string[]>([]);
  // 消息暂存：停泊到本地持久存储，稍后弹回队列（对齐 TUI /stash）
  const [stash, setStash] = useState<string[]>(() => {
    try {
      const raw = localStorage.getItem("ds_stash");
      return raw ? (JSON.parse(raw) as string[]) : [];
    } catch {
      return [];
    }
  });
  // 暂存变化时持久化到 localStorage
  useEffect(() => {
    try {
      localStorage.setItem("ds_stash", JSON.stringify(stash));
    } catch {
      // 忽略持久化失败（隐私模式 / 配额）
    }
  }, [stash]);
  /** 桌面版：加载推理强度配置 */
  useEffect(() => {
    if (!isTauri()) return;
    void getConfig().then((c) => {
      const e = c?.reasoning_effort;
      if (e && typeof e === "string") setReasoningEffort(e as ReasoningEffort);
    });
  }, [showSettings]);

  /** 切换推理强度并重启后端使 config.toml 生效 */
  const onReasoningEffortChange = useCallback(async (next: ReasoningEffort) => {
    setReasoningEffort(next);
    if (!isTauri()) return;
    try {
      await saveConfig({ reasoning_effort: next });
      await restartBackend();
    } catch (e) {
      alert(`推理强度保存失败：${(e as Error).message}`);
    }
  }, []);

  /** Shift+Tab 循环推理强度 */
  const cycleReasoningEffortHandler = useCallback(() => {
    void onReasoningEffortChange(cycleReasoningEffort(reasoningEffort));
  }, [reasoningEffort, onReasoningEffortChange]);

  /** 启动时是否已按记忆的项目根目录恢复过会话 */
  const didStartupSessionRestore = useRef(false);

  // 当前激活的线程记录
  const activeThread = threads.find((t) => t.id === activeId) ?? null;
  /** 当前资源管理器项目下的会话（标签栏仅显示这些，类似 Cursor） */
  const workspaceThreads = useMemo(() => {
    if (!rootPath) {
      return threads.filter((t) => showArchived || !t.archived);
    }
    return filterThreadsForWorkspace(threads, rootPath, showArchived);
  }, [threads, rootPath, showArchived]);
  /** Agent 实际读写的工作目录（会话创建时绑定，与左侧资源管理器可能不同） */
  const agentWorkspace = activeThread?.workspace ?? rootPath;
  /** 当前会话工作目录与资源管理器不一致（换文件夹后仍用旧会话时） */
  const workspaceMismatch = Boolean(
    activeThread && rootPath && !workspacePathsEqual(activeThread.workspace, rootPath),
  );

  /** 关闭任务/技能/历史/设置等子视图，回到聊天主界面 */
  const showChatView = useCallback(() => {
    setShowSessions(false);
    setShowSettings(false);
  }, []);

  /** 打开设置页并定位到指定分类 */
  const openSettings = useCallback((tab: SettingsTab = "models") => {
    setShowSessions(false);
    setSettingsTab(tab);
    setShowSettings(true);
  }, []);

  /** 新建会话：绑定当前（或指定）工作目录，避免 Agent 写到旧文件夹 */
  const createThread = useCallback(
    async (workspaceOverride?: string) => {
      const ws = workspaceOverride ?? rootPath ?? undefined;
      try {
        const t = await client.createThread({ model: newModel, mode: newMode, workspace: ws });
        setThreads((prev) => [t, ...prev]);
        showChatView();
        setActiveId(t.id);
        if (ws) saveLastThreadForWorkspace(ws, t.id);
      } catch (e) {
        alert(`新建会话失败：${(e as Error).message}`);
      }
    },
    [client, newModel, newMode, rootPath, showChatView],
  );

  /**
   * 打开项目文件夹：同步后端 cwd，并恢复该项目上次 Agent 或新建会话（Cursor 行为）
   */
  const openWorkspace = useCallback(
    async (dir: string) => {
      setRootPath(dir);
      editor.closeAll();
      localStorage.setItem("ds_root", dir);
      await setWorkspace(dir);
      const ok = await waitForBackend(client);
      if (!ok) {
        alert(t("workspace.backendTimeout", locale));
        setTreeTick((n) => n + 1);
        return;
      }
      setBackendUp(true);
      let list: ThreadRecord[] = [];
      try {
        list = await client.listThreads(showArchived);
        setThreads(list);
      } catch {
        list = [];
      }
      const existing = pickThreadForWorkspace(list, dir);
      showChatView();
      if (existing) {
        setActiveId(existing.id);
        saveLastThreadForWorkspace(dir, existing.id);
      } else {
        await createThread(dir);
      }
      setTreeTick((n) => n + 1);
    },
    [client, createThread, editor, locale, showArchived, showChatView],
  );

  /** 选择项目文件夹（文件对话框） */
  const chooseFolder = useCallback(async () => {
    try {
      const dir = await pickFolder();
      if (!dir) return;
      await openWorkspace(dir);
    } catch (e) {
      alert(`打开文件夹失败：${(e as Error).message}`);
    }
  }, [openWorkspace]);

  /** 打开文件：供文件树、@ 引用等调用 */
  const handleOpenFile = useCallback(
    (path: string) => {
      editor.openFile(path);
    },
    [editor],
  );

  /** 关闭文件标签 */
  const handleCloseFile = useCallback(
    (path: string) => {
      editor.closeFile(path);
    },
    [editor],
  );

  /** 从消息/Agent 事件打开文件（相对路径基于会话绑定的工作目录解析） */
  const openFileFromWorkspace = useCallback(
    (path: string) => {
      const abs = resolveWorkspacePath(agentWorkspace, path);
      if (abs) handleOpenFile(abs);
    },
    [agentWorkspace, handleOpenFile],
  );

  /** 保存当前会话 system_prompt */
  const saveSystemPrompt = useCallback(async () => {
    if (!activeId) return;
    const sp = systemPromptDraft.trim();
    try {
      await client.patchThread(activeId, { system_prompt: sp || null });
      setThreads((prev) =>
        prev.map((t) => (t.id === activeId ? { ...t, system_prompt: sp || undefined } : t)),
      );
    } catch (e) {
      alert(`保存系统提示词失败：${(e as Error).message}`);
    }
  }, [client, activeId, systemPromptDraft]);

  /** 切换界面语言 */
  const toggleLocale = useCallback(() => {
    setLocale((cur) => {
      const next: Locale = cur === "zh" ? "en" : "zh";
      saveLocale(next);
      return next;
    });
  }, []);

  // 桌面应用内：从壳层获取后端真实 token，覆盖默认值
  useEffect(() => {
    if (!isTauri()) return;
    getRuntimeToken()
      .then((tok) => {
        if (tok) {
          setFormToken(tok);
          setCfg((prev) => ({ ...prev, token: tok }));
        }
      })
      .catch(() => {
        /* 忽略 */
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 启动时若记忆了项目根目录，把后端工作目录切到该目录（重启后端）
  useEffect(() => {
    if (!isTauri() || !rootPath) return;
    setWorkspace(rootPath).catch(() => {
      /* 忽略：后端可能尚未就绪，轮询会重试 */
    });
    // 仅在挂载时执行一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 当前激活线程变化时同步 system_prompt 草稿
  useEffect(() => {
    setSystemPromptDraft(activeThread?.system_prompt ?? "");
  }, [activeThread?.id, activeThread?.system_prompt]);

  // 订阅当前会话事件（多线程缓存 + since_seq 续传）
  const conv = useConversation(cfg, activeId);
  /** 最后一条用户消息（/retry） */
  const lastUserMessage = useMemo(() => {
    for (let i = conv.items.length - 1; i >= 0; i--) {
      const it = conv.items[i]!;
      if (it.kind === "user_message" && it.text.trim()) return it.text.trim();
    }
    return null;
  }, [conv.items]);
  /** 聊天消息区滚底：发送后跳转 + 流式跟随 */
  const { ref: transcriptRef, scrollAfterSend } = useTranscriptAutoScroll(
    conv.items,
    conv.running,
    activeId,
  );

  // 自动批准 / 信任：后端仍会 SSE 推送 approval.required，前端静默代批避免重复弹窗
  useEffect(() => {
    if (!activeThread || conv.approvals.length === 0) return;
    if (!activeThread.auto_approve && !activeThread.trust_mode) return;
    const pending = conv.approvals[0];
    void conv.resolveApproval(pending.approvalId, "approve", true);
  }, [
    activeThread?.auto_approve,
    activeThread?.trust_mode,
    conv.approvals,
    conv.resolveApproval,
  ]);

  /** 用户手动审批：批准后若勾选 remember，同步本地 thread 状态 */
  const handleResolveApproval = useCallback(
    async (approvalId: string, decision: "approve" | "reject", remember: boolean) => {
      await conv.resolveApproval(approvalId, decision, remember);
      if (decision !== "approve" || !remember || !activeId) return;
      setThreads((prev) =>
        prev.map((t) => (t.id === activeId ? { ...t, auto_approve: true } : t)),
      );
    },
    [conv.resolveApproval, activeId],
  );

  /** 是否应对用户展示审批弹窗（自动批准/信任时由 effect 静默处理） */
  const showApprovalDialog =
    conv.approvals.length > 0 &&
    !activeThread?.auto_approve &&
    !activeThread?.trust_mode;

  // 回合进行中：每个文件写入完成即刷新文件树（不自动打开编辑器标签）
  useEffect(() => {
    if (conv.fileChangeTick <= 0) return;
    setTreeTick((n) => n + 1);
  }, [conv.fileChangeTick]);

  // Agent 运行中轻量轮询，兜底 shell 等方式创建的目录/文件
  useEffect(() => {
    if (!conv.running) return;
    const timer = setInterval(() => setTreeTick((n) => n + 1), 2000);
    return () => clearInterval(timer);
  }, [conv.running]);

  // 回合结束后再刷新一次文件树（不批量打开标签）
  useEffect(() => {
    if (conv.usageTick <= 0) return;
    setTreeTick((n) => n + 1);
  }, [conv.usageTick]);

  /** 探测后端健康并加载会话列表（可含已归档） */
  const refresh = useCallback(async () => {
    const ok = await client.health();
    setBackendUp(ok);
    if (ok) {
      try {
        setThreads(await client.listThreads(showArchived));
      } catch {
        setThreads([]);
      }
    }
  }, [client, showArchived]);

  // 启动时探测，并定时重试直到后端就绪
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, [refresh]);

  // 启动后：若记忆了项目根目录，自动恢复该项目的 Agent 会话（无则新建）
  useEffect(() => {
    if (didStartupSessionRestore.current || !backendUp || !rootPath || threads.length === 0) {
      return;
    }
    const active = activeId ? threads.find((th) => th.id === activeId) : null;
    if (active && workspacePathsEqual(active.workspace, rootPath)) {
      didStartupSessionRestore.current = true;
      return;
    }
    didStartupSessionRestore.current = true;
    const existing = pickThreadForWorkspace(threads, rootPath);
    if (existing) {
      showChatView();
      setActiveId(existing.id);
      saveLastThreadForWorkspace(rootPath, existing.id);
    } else if (!activeId) {
      void createThread(rootPath);
    }
  }, [backendUp, rootPath, threads, activeId, createThread, showChatView]);

  // 回合完成或切换会话时刷新用量统计
  useEffect(() => {
    if (!backendUp) return;
    client
      .getUsage()
      .then((u) => setUsage(u.totals))
      .catch(() => setUsage(null));
  }, [client, backendUp, conv.usageTick, activeId]);

  // 定期刷新 Git 工作区状态（分支、暂存/未暂存计数）
  useEffect(() => {
    if (!backendUp) return;
    client
      .getWorkspaceStatus()
      .then(setWsStatus)
      .catch(() => setWsStatus(null));
  }, [client, backendUp, conv.usageTick, rootPath]);

  /** 选中某个会话并切回聊天视图（在任务/设置等子页时点击标签也能生效） */
  const selectThread = useCallback(
    (id: string) => {
      showChatView();
      setActiveId(id);
      const thread = threads.find((th) => th.id === id);
      if (thread?.workspace) saveLastThreadForWorkspace(thread.workspace, id);
    },
    [showChatView, threads],
  );

  /** 将资源管理器根目录同步到指定工作区（切换跨项目会话时使用） */
  const syncRootPathForThread = useCallback(
    async (workspace: string): Promise<boolean> => {
      if (!workspace.trim()) return true;
      if (rootPath && workspacePathsEqual(rootPath, workspace)) return true;
      setRootPath(workspace);
      editor.closeAll();
      localStorage.setItem("ds_root", workspace);
      if (!isTauri()) {
        setTreeTick((n) => n + 1);
        return true;
      }
      await setWorkspace(workspace);
      const ok = await waitForBackend(client);
      if (!ok) {
        alert(t("workspace.backendTimeout", locale));
        return false;
      }
      setBackendUp(true);
      setTreeTick((n) => n + 1);
      return true;
    },
    [client, editor, locale, rootPath],
  );

  /**
   * 切换会话；可选同步左侧项目文件夹到该会话绑定的工作区
   */
  const switchToThread = useCallback(
    async (id: string, options?: { syncWorkspace?: boolean }) => {
      let thread = threads.find((th) => th.id === id);
      // 搜索到的会话可能不在当前标签列表，需拉取详情以获知 workspace
      if (!thread && options?.syncWorkspace) {
        try {
          const detail = await client.getThread(id);
          thread = detail.thread;
          setThreads((prev) =>
            prev.some((t) => t.id === id) ? prev : [detail.thread, ...prev],
          );
        } catch {
          /* 仍尝试切换 id */
        }
      }
      if (options?.syncWorkspace && thread?.workspace) {
        const ok = await syncRootPathForThread(thread.workspace);
        if (!ok) return;
      }
      selectThread(id);
    },
    [client, selectThread, syncRootPathForThread, threads],
  );

  /** 修改当前会话的模型、模式或安全开关 */
  const onChangeThreadField = useCallback(
    async (patch: {
      model?: string;
      mode?: string;
      allow_shell?: boolean;
      trust_mode?: boolean;
      auto_approve?: boolean;
    }) => {
      if (!activeId) return;
      try {
        await client.patchThread(activeId, patch);
        setThreads((prev) => prev.map((t) => (t.id === activeId ? { ...t, ...patch } : t)));
      } catch (e) {
        alert(`更新会话失败：${(e as Error).message}`);
      }
    },
    [client, activeId],
  );

  /** 关闭（归档）会话：从列表移除（除非正在查看归档），若是当前会话则清空 */
  const onCloseThread = useCallback(
    async (id: string) => {
      try {
        await client.patchThread(id, { archived: true });
        if (showArchived) {
          // 归档视图下：保留并标记为已归档
          setThreads((prev) => prev.map((t) => (t.id === id ? { ...t, archived: true } : t)));
        } else {
          setThreads((prev) => prev.filter((t) => t.id !== id));
        }
        setActiveId((cur) => (cur === id ? null : cur));
      } catch (e) {
        alert(`关闭会话失败：${(e as Error).message}`);
      }
    },
    [client, showArchived],
  );

  /** 恢复已归档会话 */
  const onRestoreThread = useCallback(
    async (id: string) => {
      try {
        await client.patchThread(id, { archived: false });
        setThreads((prev) => prev.map((t) => (t.id === id ? { ...t, archived: false } : t)));
      } catch (e) {
        alert(`恢复会话失败：${(e as Error).message}`);
      }
    },
    [client],
  );

  /** 重命名会话 */
  const onRenameThread = useCallback(
    async (id: string, current: string) => {
      const next = window.prompt("会话名称", current);
      if (next == null) return;
      const title = next.trim();
      if (!title || title === current) return;
      try {
        await client.patchThread(id, { title });
        setThreads((prev) => prev.map((t) => (t.id === id ? { ...t, title } : t)));
      } catch (e) {
        alert(`重命名失败：${(e as Error).message}`);
      }
    },
    [client],
  );

  /** 发送普通用户消息（不含斜杠拦截） */
  const sendPrompt = useCallback(
    async (text: string) => {
      if (!activeId) return;
      scrollAfterSend();
      const threadId = activeId;
      const shouldAutoTitle = activeThread && isUntitledThread(activeThread);
      const optimisticTitle = shouldAutoTitle ? deriveThreadTitleFromMessage(text) : null;
      try {
        const { thread } = await client.startTurn(threadId, { prompt: text });
        setThreads((prev) =>
          prev.map((th) => {
            if (th.id !== threadId) return th;
            const merged = { ...th, ...thread };
            if (shouldAutoTitle && optimisticTitle && isUntitledThread(merged)) {
              return { ...merged, title: optimisticTitle };
            }
            return merged;
          }),
        );
      } catch (e) {
        alert(`发送失败：${(e as Error).message}`);
      }
    },
    [client, activeId, activeThread, scrollAfterSend],
  );

  /** 入队一条消息（去除首尾空白后非空才入队） */
  const enqueue = useCallback((text: string) => {
    const v = text.trim();
    if (v) setQueued((prev) => [...prev, v]);
  }, []);

  /** 队列进行中自动排空：回合结束且有排队项时，按序发送下一条 */
  useEffect(() => {
    if (conv.running) return;
    if (queued.length === 0) return;
    if (!activeId) return;
    const [next, ...rest] = queued;
    setQueued(rest);
    void sendPrompt(next);
  }, [conv.running, queued, activeId, sendPrompt]);

  /** 执行斜杠命令：返回 true 表示已处理（不再作为普通消息发送） */
  const runSlashCommand = useCallback(
    async (raw: string): Promise<boolean> => {
      return executeSlashCommand(raw, {
        client,
        locale,
        models: MODELS,
        modes: MODES,
        activeId,
        activeThread,
        rootPath,
        lastUserMessage,
        setShowDiff,
        setShowSessions,
        setShowUsage,
        setShowSnapshots,
        afterRestore: () => {
          // 还原后刷新文件树（打开的文件可重新打开以加载新内容）
          setTreeTick((n) => n + 1);
        },
        openSettings,
        onChangeThreadField,
        refresh,
        setActiveId,
        openWorkspace,
        chooseFolder,
        onSend: sendPrompt,
        insertAttachmentPath: (rel) => composerInsertRef.current?.(rel),
        enqueue,
        clearQueue: () => setQueued([]),
        stashQueue: () =>
          setQueued((q) => {
            if (q.length > 0) setStash((s) => [...s, ...q]);
            return [];
          }),
        popStash: () =>
          setStash((s) => {
            if (s.length > 0) setQueued((q) => [...q, ...s]);
            return [];
          }),
        clearStash: () => setStash([]),
      });
    },
    [
      client,
      locale,
      activeId,
      activeThread,
      rootPath,
      lastUserMessage,
      openSettings,
      onChangeThreadField,
      refresh,
      openWorkspace,
      chooseFolder,
      sendPrompt,
      enqueue,
    ],
  );

  /** 发送消息（支持斜杠命令） */
  const onSend = useCallback(
    async (text: string) => {
      if (!activeId) return;
      if (text.trim().startsWith("/")) {
        await runSlashCommand(text);
        return;
      }
      await sendPrompt(text);
    },
    [activeId, runSlashCommand, sendPrompt],
  );

  /** 转向 */
  const onSteer = useCallback(
    async (text: string) => {
      if (!activeId) return;
      scrollAfterSend();
      const turnId = conv.currentTurnId;
      if (!turnId) {
        await onSend(text);
        return;
      }
      const trySteer = () => client.steerTurn(activeId, turnId, text);
      try {
        await trySteer();
      } catch (e) {
        const msg = (e as Error).message;
        const stale =
          msg.includes("Thread is not loaded") ||
          msg.includes("No active turn") ||
          msg.includes("not active on thread");
        if (!stale) {
          alert(`转向失败：${msg}`);
          return;
        }
        // 后端引擎未加载或回合状态过期：先 resume 再重试，仍失败则作为新消息发送
        try {
          await client.resumeThread(activeId);
          await trySteer();
        } catch {
          try {
            const { thread } = await client.startTurn(activeId, { prompt: text });
            setThreads((prev) =>
              prev.map((th) => (th.id === activeId ? { ...th, ...thread } : th)),
            );
          } catch (e2) {
            alert(`发送失败：${(e2 as Error).message}`);
          }
        }
      }
    },
    [client, activeId, conv.currentTurnId, onSend, scrollAfterSend],
  );

  /** 回合结束后按 alwaysApply 规则检测缺口（如 README），并自动/手动跟进 */
  const ruleCompliance = useRuleCompliance({
    rootPath: agentWorkspace,
    running: conv.running,
    lastTurnChangedPaths: conv.lastTurnChangedPaths,
    onFollowUp: onSend,
    locale,
  });

  /** 打断 */
  const onInterrupt = useCallback(async () => {
    if (!activeId || !conv.currentTurnId) return;
    try {
      await client.interruptTurn(activeId, conv.currentTurnId);
    } catch (e) {
      alert(`打断失败：${(e as Error).message}`);
    }
  }, [client, activeId, conv.currentTurnId]);

  /**
   * 切换右栏历史会话 / 中栏设置：设置打开时占中间编辑器区域，Chat 保持可见。
   */
  const toggleView = useCallback(
    (view: "sessions" | "settings" | null) => {
      const isOpen =
        (view === "sessions" && showSessions) ||
        (view === "settings" && showSettings);
      const target = isOpen ? null : view;
      setShowSessions(target === "sessions");
      if (target === "settings") {
        openSettings("models");
      } else {
        setShowSettings(false);
      }
    },
    [showSessions, showSettings, openSettings],
  );

  /** 历史会话恢复成功：刷新列表并切换到新线程 */
  const onSessionResumed = useCallback(
    async (threadId: string) => {
      setShowSessions(false);
      try {
        const detail = await client.getThread(threadId);
        if (detail.thread.workspace) {
          await syncRootPathForThread(detail.thread.workspace);
        }
        setThreads((prev) => {
          const exists = prev.some((t) => t.id === threadId);
          if (exists) {
            return prev.map((t) => (t.id === threadId ? detail.thread : t));
          }
          return [detail.thread, ...prev];
        });
      } catch {
        await refresh();
      }
      selectThread(threadId);
    },
    [client, refresh, selectThread, syncRootPathForThread],
  );

  // 全局快捷键：Ctrl/Cmd+K 命令面板、Ctrl/Cmd+P 快速打开
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setShowPalette((v) => !v);
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p") {
        e.preventDefault();
        if (e.shiftKey) setShowThreadSearch((v) => !v);
        else setShowQuickOpen((v) => !v);
      }
      // Ctrl+` 切换底部集成终端（仿 VS Code）
      if ((e.ctrlKey || e.metaKey) && e.key === "`") {
        e.preventDefault();
        toggleTerminal();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleTerminal]);

  /** 命令面板可执行项 */
  const paletteCommands = useMemo<PaletteCommand[]>(
    () => [
      { id: "new", title: "新建会话", hint: "＋", run: () => void createThread() },
      { id: "sessions", title: "浏览历史会话", hint: "/sessions", run: () => toggleView("sessions") },
      { id: "diff", title: "查看工作区变更", hint: "/diff", run: () => setShowDiff(true) },
      {
        id: "review",
        title: "代码审查当前改动",
        hint: "/review",
        run: () => runSlashCommand("/review"),
        disabled: !activeId,
      },
      {
        id: "compact",
        title: "压缩上下文",
        hint: "/compact",
        run: () => runSlashCommand("/compact"),
        disabled: !activeId,
      },
      {
        id: "fork",
        title: "复刻当前会话",
        hint: "/fork",
        run: () => runSlashCommand("/fork"),
        disabled: !activeId,
      },
      {
        id: "mode",
        title: "切换 Agent 模式",
        hint: "/mode",
        run: () => runSlashCommand("/mode"),
        disabled: !activeId,
      },
      {
        id: "trust",
        title: "切换信任模式",
        hint: "/trust",
        run: () => runSlashCommand("/trust"),
        disabled: !activeId,
      },
      {
        id: "export",
        title: "导出当前会话",
        hint: "/export",
        run: () => runSlashCommand("/export"),
        disabled: !activeId,
      },
      {
        id: "tokens",
        title: "Token 用量",
        hint: "/tokens",
        run: () => runSlashCommand("/tokens"),
        disabled: !activeId,
      },
      {
        id: "workspace",
        title: "切换工作区",
        hint: "/workspace",
        run: () => runSlashCommand("/workspace"),
      },
      { id: "tasks", title: "任务 / 自动化", hint: "📋", run: () => openSettings("tasks") },
      { id: "skills", title: "技能 / MCP", hint: "🧩", run: () => openSettings("skills") },
      { id: "rules", title: "项目规则", hint: "📜", run: () => openSettings("rules"), disabled: !rootPath },
      { id: "open", title: "打开项目文件夹", hint: "📁", run: chooseFolder },
      {
        id: "quickopen",
        title: t("palette.quickOpen", locale),
        hint: "Ctrl+P",
        run: () => setShowQuickOpen(true),
        disabled: !rootPath,
      },
      {
        id: "threadsearch",
        title: t("search.threadsTitle", locale),
        hint: "Ctrl+Shift+P",
        run: () => setShowThreadSearch(true),
      },
      { id: "settings", title: "设置", hint: "⚙", run: () => toggleView("settings") },
      { id: "conn", title: "后端连接", hint: "🔌", run: () => openSettings("connection") },
    ],
    [createThread, chooseFolder, runSlashCommand, toggleView, openSettings, activeId, locale, rootPath],
  );

  return (
    <div className="app-shell">
      {/* 顶栏菜单（仿 Cursor File / Edit / View …） */}
      <TitleMenuBar
        locale={locale}
        sidebarOpen={sidebarOpen}
        hasFolder={!!rootPath}
        onOpenFolder={() => void chooseFolder()}
        onQuickOpen={() => setShowQuickOpen(true)}
        onCommandPalette={() => setShowPalette(true)}
        onSearchChats={() => setShowThreadSearch(true)}
        onOpenSettings={openSettings}
        onToggleSidebar={() => setSidebarOpen((v) => !v)}
        onToggleChat={() => setChatOpen((v) => !v)}
        chatOpen={chatOpen}
        onNewChat={() => void createThread()}
        onShowDiff={() => setShowDiff(true)}
        onToggleSessions={() => toggleView("sessions")}
        onToggleTerminal={toggleTerminal}
        settingsOpen={showSettings}
        onToggleSettings={() => toggleView("settings")}
        onZoomIn={uiZoom.zoomIn}
        onZoomOut={uiZoom.zoomOut}
        onZoomReset={uiZoom.zoomReset}
      />
    <div className="ide-resize-host">
    <div className={`ide${sidebarOpen ? "" : " sidebar-collapsed"}${chatOpen ? "" : " chat-collapsed"}`}>
      {/* 左栏：资源管理器（文件树）；侧栏开关已移至顶栏菜单栏 */}
      <aside className={`pane-left${sidebarOpen ? "" : " collapsed"}`}>
        <div className="pane-head">
          <div className="pane-head-left">
            <span className="pane-title">{t("app.explorer", locale)}</span>
          </div>
          <div className="pane-head-btns">
            {rootPath && (
              <button
                className="btn-mini"
                onClick={() => setTreeTick((n) => n + 1)}
                title={t("app.refreshTree", locale)}
              >
                ⟳
              </button>
            )}
            <button className="btn-mini" onClick={chooseFolder} title={t("app.openFolder", locale)}>
              {t("app.openFolder", locale)}
            </button>
          </div>
        </div>
        {rootPath ? (
          <>
            <div className="root-name" title={rootPath}>
              {rootPath.split(/[\\/]/).pop()}
            </div>
            <FileTree
              rootPath={rootPath}
              activePath={editor.activeFile}
              onOpenFile={handleOpenFile}
              reloadToken={treeTick}
              onChanged={() => setTreeTick((n) => n + 1)}
              onDeleted={(p) => {
                handleCloseFile(p);
              }}
              onRenamed={(from, to) => editor.renameFile(from, to)}
            />
          </>
        ) : (
          <div className="pane-placeholder">
            {isTauri() ? t("app.noFolder", locale) : t("app.noFolderWeb", locale)}
          </div>
        )}
      </aside>

      {/* 中栏：代码编辑 或 设置（Cursor 风格：设置占中间主区域），底部可停靠终端 */}
      <section className="pane-center">
        <div className="center-main">
        {showSettings ? (
          <SettingsView
            client={client}
            locale={locale}
            tab={settingsTab}
            onTabChange={setSettingsTab}
            onBack={() => setShowSettings(false)}
            onSaved={refresh}
            rootPath={rootPath}
            backendUp={backendUp}
            formUrl={formUrl}
            formToken={formToken}
            onFormUrlChange={setFormUrl}
            onFormTokenChange={setFormToken}
            onApplyConnection={() => {
              applyConn();
              void refresh();
            }}
            showArchived={showArchived}
            onShowArchivedChange={setShowArchived}
          />
        ) : (
          <EditorPanel
            openFiles={editor.openFiles}
            activeFile={editor.activeFile}
            onSelectFile={editor.setActiveFile}
            onCloseFile={handleCloseFile}
            onCloseOthers={editor.closeOthers}
            onCloseToRight={editor.closeToRight}
            onCloseToLeft={editor.closeToLeft}
            onCloseAll={editor.closeAll}
            locale={locale}
            hasFolder={!!rootPath}
            onOpenFolder={() => void chooseFolder()}
            onQuickOpen={() => setShowQuickOpen(true)}
            onCommandPalette={() => setShowPalette(true)}
            onSearchChats={() => setShowThreadSearch(true)}
            onOpenSettings={() => openSettings("models")}
            onNewChat={() => void createThread()}
            workspaceRoot={rootPath}
          />
        )}
        </div>
      </section>

      {/* 底部集成终端（仿 Cursor）：作为 .ide 网格底部行，跨侧栏+编辑器，往左到底；首次打开后保持挂载，仅切显隐以保活 PTY */}
      {terminalEverOpened && (
        <div
          className={`terminal-dock${showTerminal ? "" : " is-hidden"}`}
          style={{ height: showTerminal ? termHeight : 0 }}
        >
          <div className="terminal-dock-resizer" onMouseDown={onTermResizeStart} />
          <div className="terminal-dock-head">
            <span className="terminal-dock-title">{locale === "zh" ? "终端" : "Terminal"}</span>
            <button
              type="button"
              className="terminal-dock-close"
              title={locale === "zh" ? "关闭终端 (Ctrl+`)" : "Close terminal (Ctrl+`)"}
              onClick={() => setShowTerminal(false)}
            >
              ×
            </button>
          </div>
          <div className="terminal-dock-body">
            <TerminalPanel locale={locale} workspace={rootPath} fill />
          </div>
        </div>
      )}

      {/* 右栏：聊天（Agent 面板） */}
      <aside className={`pane-right${chatOpen ? "" : " collapsed"}`}>
        <div className="chat-top chat-top-row">
          <div className="chat-tabs">
            <div className="tabs-scroll">
              {workspaceThreads.map((t) => {
                const cls = [
                  "chat-tab",
                  t.id === activeId ? "active" : "",
                  t.archived ? "archived" : "",
                ]
                  .filter(Boolean)
                  .join(" ");
                return (
                  <div
                    key={t.id}
                    className={cls}
                    onClick={() => selectThread(t.id)}
                    onDoubleClick={() =>
                      onRenameThread(t.id, formatThreadTabTitle(t, locale))
                    }
                    title={`${formatThreadTabTitle(t, locale)}${t.archived ? "（已归档）" : "（双击重命名）"}`}
                  >
                    <span className="chat-tab-name">{formatThreadTabTitle(t, locale)}</span>
                    {t.archived ? (
                      <button
                        type="button"
                        className="chat-tab-close"
                        title="恢复会话"
                        onClick={(e) => {
                          e.stopPropagation();
                          onRestoreThread(t.id);
                        }}
                      >
                        ⟲
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="chat-tab-close"
                        title="关闭会话"
                        onClick={(e) => {
                          e.stopPropagation();
                          onCloseThread(t.id);
                        }}
                      >
                        ×
                      </button>
                    )}
                  </div>
                );
              })}
              <button type="button" className="tab-add" onClick={() => void createThread()} title="新建会话">
                +
              </button>
            </div>
          </div>
          <div className="chat-header-row">
            <div className="tabs-actions">
              <button
                type="button"
                className={`icon-btn${showSessions ? " active" : ""}`}
                onClick={() => toggleView("sessions")}
                title={t("status.history", locale)}
              >
                <svg viewBox="0 0 24 24" width="15" height="15" aria-hidden fill="currentColor">
                  <path d="M13 3a9 9 0 1 0 8.94 10H19.9A7 7 0 1 1 13 5v3l4-4-4-4v3zm-1 5v5l4.25 2.52.75-1.23-3.5-2.08V8H12z" />
                </svg>
              </button>
              <button
                type="button"
                className="icon-btn"
                onClick={() => setShowThreadSearch(true)}
                title={t("search.threadsTitle", locale)}
              >
                ⌕
              </button>
              <div className="cmd-wrap">
                <button
                  type="button"
                  className={`icon-btn${showCmd ? " active" : ""}`}
                  onClick={() => setShowCmd((v) => !v)}
                  title="命令"
                  disabled={!activeId}
                >
                  /
                </button>
                {showCmd && (
                  <div className="cmd-menu" onMouseLeave={() => setShowCmd(false)}>
                    <button onClick={() => { setShowCmd(false); runSlashCommand("/compact"); }}>
                      压缩上下文 <code>/compact</code>
                    </button>
                    <button onClick={() => { setShowCmd(false); runSlashCommand("/fork"); }}>
                      复刻会话 <code>/fork</code>
                    </button>
                    <button onClick={() => { setShowCmd(false); runSlashCommand("/review"); }}>
                      代码审查 <code>/review</code>
                    </button>
                    <button onClick={() => { setShowCmd(false); setShowDiff(true); }}>
                      查看变更 <code>/diff</code>
                    </button>
                  </div>
                )}
              </div>
              <button
                type="button"
                className={`dot-btn${backendUp ? " dot-on" : " dot-off"}`}
                title={backendUp ? t("app.backendOnline", locale) : t("app.backendOfflineShort", locale)}
                onClick={() => openSettings("connection")}
              />
            </div>
          </div>
        </div>

        {backendUp === false && (
          <div className="banner banner-warn">{t("app.backendOffline", locale)}</div>
        )}

        {workspaceMismatch && (
          <div className="banner banner-warn workspace-banner">
            <span>{t("workspace.mismatch", locale)}</span>
            <button type="button" className="banner-action" onClick={() => void createThread()}>
              {t("workspace.newThread", locale)}
            </button>
          </div>
        )}

        {showSessions ? (
          <AgentHistoryPanel
            client={client}
            locale={locale}
            threads={workspaceThreads}
            activeId={activeId}
            onSelectThread={(id) => {
              selectThread(id);
              setShowSessions(false);
            }}
            onNewChat={() => {
              void createThread();
              setShowSessions(false);
            }}
            onSessionResumed={(threadId) => {
              onSessionResumed(threadId);
              setShowSessions(false);
            }}
          />
        ) : (
          <div className="chat-main">
            {!activeId ? (
              <div className="placeholder">{t("app.selectThread", locale)}</div>
            ) : (
              <>
                <NoticeList
                  notices={conv.notices}
                  locale={locale}
                  onDismiss={conv.dismissNotice}
                />
                <RuleComplianceBanner
                  notice={ruleCompliance.notice}
                  locale={locale}
                  onDismiss={ruleCompliance.dismiss}
                  onRetry={() => void ruleCompliance.retryFollowUp()}
                />
                <div className="transcript" ref={transcriptRef}>
                  {conv.items.length === 0 && (
                    <div className="placeholder">{t("app.firstMessage", locale)}</div>
                  )}
                  {conv.items.map((it) => (
                    <MessageItem
                      key={it.id}
                      item={it}
                      locale={locale}
                      onOpenFile={openFileFromWorkspace}
                    />
                  ))}
                </div>
              </>
            )}

            {/* 底部输入区 + 模型/模式/安全开关（仿 Cursor 置底） */}
            <div className="chat-footer">
              {activeThread && showSystemPrompt && (
                <div className="thread-prompt-edit">
                  <textarea
                    className="thread-prompt-area"
                    placeholder={t("thread.systemPromptPh", locale)}
                    value={systemPromptDraft}
                    onChange={(e) => setSystemPromptDraft(e.target.value)}
                    rows={2}
                  />
                  <button type="button" className="btn-mini" onClick={() => void saveSystemPrompt()}>
                    {locale === "zh" ? "保存" : "Save"}
                  </button>
                </div>
              )}
              <QueueBar
                locale={locale}
                queued={queued}
                stash={stash}
                onEditQueued={(i, text) =>
                  setQueued((q) => {
                    const v = text.trim();
                    if (!v) return q.filter((_, idx) => idx !== i);
                    return q.map((item, idx) => (idx === i ? v : item));
                  })
                }
                onDropQueued={(i) => setQueued((q) => q.filter((_, idx) => idx !== i))}
                onClearQueue={() => setQueued([])}
                onStashQueue={() =>
                  setQueued((q) => {
                    if (q.length > 0) setStash((s) => [...s, ...q]);
                    return [];
                  })
                }
                onPopStash={() =>
                  setStash((s) => {
                    if (s.length > 0) setQueued((q) => [...q, ...s]);
                    return [];
                  })
                }
                onClearStash={() => setStash([])}
              />
              <Composer
                disabled={!activeId}
                running={conv.running}
                onSend={onSend}
                onSteer={onSteer}
                onInterrupt={onInterrupt}
                rootPath={rootPath}
                locale={locale}
                models={MODELS}
                modes={MODES}
                model={activeThread ? activeThread.model : newModel}
                mode={activeThread ? activeThread.mode : newMode}
                onModelChange={(v) =>
                  activeThread ? void onChangeThreadField({ model: v }) : setNewModel(v)
                }
                onModeChange={(v) =>
                  activeThread ? void onChangeThreadField({ mode: v }) : setNewMode(v)
                }
                activeThread={activeThread}
                onChangeThreadField={onChangeThreadField}
                showSystemPrompt={showSystemPrompt}
                onToggleSystemPrompt={() => setShowSystemPrompt((v) => !v)}
                reasoningEffort={reasoningEffort}
                onReasoningEffortChange={(v) => void onReasoningEffortChange(v)}
                onCycleReasoningEffort={cycleReasoningEffortHandler}
                onRegisterInsert={(fn) => {
                  composerInsertRef.current = fn;
                }}
              />
            </div>
          </div>
        )}
      </aside>
    </div>
    {/* 分栏拖拽把手：左栏与 Chat 面板 */}
    {sidebarOpen && (
      <div
        className="panel-resizer panel-resizer-left"
        title="拖拽调整资源管理器宽度"
        onMouseDown={panels.startSidebarDrag}
      />
    )}
    {chatOpen && (
    <div
      className="panel-resizer panel-resizer-right"
      title="拖拽调整 Chat 宽度"
      onMouseDown={panels.startChatDrag}
    />
    )}
    </div>

    {/* 浮层/模态：置于 .ide 网格外，避免 grid 自动占位挡住 Chat 底栏点击 */}
    {showDiff && <DiffModal rootPath={rootPath} onClose={() => setShowDiff(false)} />}

    {showSnapshots && (
      <SnapshotsModal
        client={client}
        locale={locale}
        threadId={activeId}
        onClose={() => setShowSnapshots(false)}
        onRestored={() => setTreeTick((n) => n + 1)}
      />
    )}

    {showUsage && (
      <UsageModal
        client={client}
        locale={locale}
        activeThreadId={activeId}
        onClose={() => setShowUsage(false)}
      />
    )}

    {showPalette && (
      <CommandPalette commands={paletteCommands} onClose={() => setShowPalette(false)} />
    )}

    {showQuickOpen && (
      <QuickOpen
        rootPath={rootPath}
        onOpen={handleOpenFile}
        onClose={() => setShowQuickOpen(false)}
      />
    )}

    {showThreadSearch && (
      <ThreadSearch
        client={client}
        locale={locale}
        onSelect={(id) => void switchToThread(id, { syncWorkspace: true })}
        onClose={() => setShowThreadSearch(false)}
      />
    )}

    {showApprovalDialog && (
      <ApprovalDialog approval={conv.approvals[0]} onDecide={handleResolveApproval} />
    )}

    {/* 底部状态栏（仿 VS Code / Cursor） */}
    <footer className="status-bar">
      <span className="status-item" title={rootPath ?? ""}>
        {rootPath ? rootPath.split(/[\\/]/).pop() : "未打开文件夹"}
        {wsStatus?.git_repo && wsStatus.branch && (
          <span className="status-git">
            {" "}⎇ {wsStatus.branch}
            {(wsStatus.staged + wsStatus.unstaged + wsStatus.untracked) > 0 &&
              ` (+${wsStatus.staged + wsStatus.unstaged + wsStatus.untracked})`}
          </span>
        )}
      </span>
      <span className="status-item">
        {editor.activeFile
          ? editor.activeFile.split(/[\\/]/).pop()
          : t("app.noOpenFile", locale)}
        {editor.openFiles.length > 1 ? ` (+${editor.openFiles.length - 1})` : ""}
      </span>
      <span className="status-spacer" />
      <StatusZoom
        label={uiZoom.zoomLabel}
        onZoomIn={uiZoom.zoomIn}
        onZoomOut={uiZoom.zoomOut}
        onReset={uiZoom.zoomReset}
        locale={locale}
      />
      <button type="button" className="status-item status-locale" onClick={toggleLocale} title="Language">
        {locale === "zh" ? "中文" : "EN"}
      </button>
      <button
        type="button"
        className="status-item status-reset"
        onClick={() => {
          panels.resetLayout();
          setSidebarOpen(true);
        }}
        title={locale === "zh" ? "重置三栏布局（文件树+编辑器+Chat）" : "Reset panel layout"}
      >
        ⊞
      </button>
      {usage && (
        <>
          <span className="status-item">
            {usage.input_tokens.toLocaleString()} in · {usage.output_tokens.toLocaleString()} out
          </span>
          <span className="status-item">${usage.cost_usd.toFixed(4)}</span>
        </>
      )}
      <span className={`status-item${backendUp ? " status-ok" : " status-warn"}`}>
        {backendUp ? t("app.backendOnline", locale) : t("app.backendOfflineShort", locale)}
      </span>
    </footer>
    </div>
  );
}
