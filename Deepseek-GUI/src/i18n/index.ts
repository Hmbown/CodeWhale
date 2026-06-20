// 国际化：中英文 UI 文案（轻量 key-value，无外部依赖）

export type Locale = "zh" | "en";

const LOCALE_KEY = "ds_locale";

/** 从 localStorage 读取语言偏好，默认中文 */
export function loadLocale(): Locale {
  try {
    const v = localStorage.getItem(LOCALE_KEY);
    if (v === "en" || v === "zh") return v;
  } catch {
    /* 忽略 */
  }
  return "zh";
}

/** 持久化语言偏好 */
export function saveLocale(locale: Locale): void {
  localStorage.setItem(LOCALE_KEY, locale);
}

/** 文案表 */
const MESSAGES: Record<Locale, Record<string, string>> = {
  zh: {
    "app.chat": "Chat",
    "app.explorer": "资源管理器",
    "app.openFolder": "打开",
    "app.refreshTree": "刷新文件树",
    "app.noFolder": "点击「打开」选择项目文件夹。",
    "app.noFolderWeb": "文件浏览仅在桌面应用内可用。",
    "app.newThread": "新建会话",
    "app.selectThread": "新建或选择一个会话开始对话。",
    "app.firstMessage": "发送第一条消息试试。",
    "app.backendOffline": "后端离线。壳会自动拉起并重试；也可手动运行 deepseek serve --http。",
    "app.noOpenFile": "无打开文件",
    "app.noFolderStatus": "未打开文件夹",
    "app.backendOnline": "后端在线",
    "app.backendOfflineShort": "后端离线",
    "status.history": "历史会话",
    "status.settings": "设置",
    "thread.shell": "Shell",
    "thread.trust": "信任",
    "thread.autoApprove": "自动批准",
    "thread.systemPrompt": "系统提示词",
    "thread.systemPromptPh": "可选：为本会话设定 system_prompt…",
    "thread.defaultTitle": "新对话",
    "notice.sandbox": "沙箱拒绝",
    "notice.coherence": "上下文一致性",
    "rule.title": "项目规则待完成",
    "rule.readmeDetail": "本轮修改了代码，但未更新 README「更新记录」。",
    "rule.commentsDetail": "本轮修改了源代码，请按规范补充中文注释。",
    "rule.databaseDetail": "本轮涉及表结构/Entity 变更，但未输出建表或迁移 SQL。",
    "rule.testsDetail": "本轮修改了代码，但未补充或更新测试。",
    "rule.apiDetail": "本轮修改了 API/路由，但未同步接口文档。",
    "rule.genericDetail": "存在尚未完成的项目规则要求。",
    "rule.fromRules": "相关规则",
    "rule.retryReadme": "补充 README",
    "rule.retryComments": "补充注释",
    "rule.retryDatabase": "输出 SQL",
    "rule.retryTests": "补充测试",
    "rule.retryApi": "更新 API 文档",
    "rule.retryGeneric": "立即处理",
    "rule.retryAll": "一键处理全部",
    "rule.retry": "立即补充 README",
    "rule.dismiss": "稍后处理",
    "search.threads": "搜索会话…",
    "search.threadsTitle": "搜索会话 (Ctrl+Shift+P)",
    "palette.quickOpen": "快速打开文件",
    "editor.pickFile": "从左侧选择一个文件查看或编辑。",
    "workspace.mismatch":
      "当前会话绑定的工作目录与左侧资源管理器不一致，Agent 会在旧目录读写文件。",
    "workspace.newThread": "为当前文件夹新建会话",
    "workspace.backendTimeout": "后端重启超时，请稍后手动点击 + 新建会话。",
  },
  en: {
    "app.chat": "Chat",
    "app.explorer": "Explorer",
    "app.openFolder": "Open",
    "app.refreshTree": "Refresh file tree",
    "app.noFolder": "Click Open to choose a project folder.",
    "app.noFolderWeb": "File browser is available in the desktop app only.",
    "app.newThread": "New chat",
    "app.selectThread": "Create or select a chat to start.",
    "app.firstMessage": "Send your first message.",
    "app.backendOffline":
      "Backend offline. The shell will retry; or run deepseek serve --http manually.",
    "app.noOpenFile": "No open file",
    "app.noFolderStatus": "No folder open",
    "app.backendOnline": "Backend online",
    "app.backendOfflineShort": "Backend offline",
    "status.history": "History",
    "status.settings": "Settings",
    "thread.shell": "Shell",
    "thread.trust": "Trust",
    "thread.autoApprove": "Auto-approve",
    "thread.systemPrompt": "System prompt",
    "thread.systemPromptPh": "Optional system_prompt for this thread…",
    "thread.defaultTitle": "New chat",
    "notice.sandbox": "Sandbox denied",
    "notice.coherence": "Coherence",
    "rule.title": "Project rule pending",
    "rule.readmeDetail": "Code changed this turn but README changelog was not updated.",
    "rule.commentsDetail": "Source files changed; add required comments per project rules.",
    "rule.databaseDetail": "Schema/entity changed but migration DDL was not provided.",
    "rule.testsDetail": "Code changed without test updates.",
    "rule.apiDetail": "API/routes changed but API docs were not updated.",
    "rule.genericDetail": "A mandatory project rule was not satisfied.",
    "rule.fromRules": "Rules",
    "rule.retryReadme": "Update README",
    "rule.retryComments": "Add comments",
    "rule.retryDatabase": "Write SQL",
    "rule.retryTests": "Add tests",
    "rule.retryApi": "Update API docs",
    "rule.retryGeneric": "Fix now",
    "rule.retryAll": "Fix all",
    "rule.retry": "Update README now",
    "rule.dismiss": "Dismiss",
    "search.threads": "Search chats…",
    "search.threadsTitle": "Search chats (Ctrl+Shift+P)",
    "palette.quickOpen": "Quick open file",
    "editor.pickFile": "Pick a file from the explorer to edit.",
    "workspace.mismatch":
      "This chat is bound to a different folder than the explorer; the agent will read/write the old folder.",
    "workspace.newThread": "New chat for this folder",
    "workspace.backendTimeout": "Backend restart timed out. Click + to create a new chat.",
  },
};

/** 取当前语言的文案；缺 key 时回退中文再回退 key 本身 */
export function t(key: string, locale: Locale): string {
  return MESSAGES[locale][key] ?? MESSAGES.zh[key] ?? key;
}
