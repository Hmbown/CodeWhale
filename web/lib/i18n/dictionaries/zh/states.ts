import type { StatesDict } from "../types";

/**
 * Simplified-Chinese dictionary for shared surface states: empty, loading,
 * error, retry, recovery, not-found, and the connection banner.
 */
export const states: StatesDict = {
  loadingLabel: "加载中…",
  emptyTitle: "这里还没有内容",
  emptyBody: "暂时没有可显示的记录。这里不会用编造的内容填充。",
  errorTitle: "页面没有加载完成",
  errorBody: "中途出了问题。你的操作没有丢失；请重试，如果持续失败，请报告给我们。",
  retry: "重试",
  reload: "重新加载页面",
  homeLink: "返回首页",
  docsIndexLink: "打开文档目录",
  notFoundTitle: "这个地址没有页面",
  notFoundBody: "链接可能已过期，或页面已迁移。文档目录列出了当前所有页面。",
  unavailableTitle: "实时记录尚未加载",
  unavailableBody: "数据源没有响应上一次刷新，或者此页面自构建以来尚未刷新。这里不会用编造的内容填充。",

  offlineTitle: "你已离线",
  offlineBody: "操作已暂停，直到网络恢复。此处显示的内容不会刷新。",
  reconnectingTitle: "正在重新连接…",
  reconnectingBody: "正在检查连接（第 {attempt} 次）。",
  degradedTitle: "连接不稳定",
  degradedBody: "服务器没有响应上一次检查。你看到的内容可能已过时。",
  onlineTitle: "已恢复在线",
  onlineBody: "连接已恢复。",
  retryNow: "立即重试",
  dismiss: "关闭",
  lastChecked: "上次检查 {time}",
};
