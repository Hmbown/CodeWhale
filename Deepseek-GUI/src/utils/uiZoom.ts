// 全局 UI 缩放（仿 Cursor / VS Code：整窗缩放，按 zoom level 步进约 20%）

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "../api/tauri";

/** 缩放级别持久化键（整数 level，0 = 100%） */
export const UI_ZOOM_LEVEL_STORAGE_KEY = "ds_ui_zoom_level";
/** 旧版比例键（兼容迁移） */
const UI_ZOOM_LEGACY_SCALE_KEY = "ds_ui_zoom";

/** 与 VS Code / Cursor 一致：每级约 20%（1.2^level） */
export const UI_ZOOM_FACTOR = 1.2;
export const UI_ZOOM_LEVEL_MIN = -5;
export const UI_ZOOM_LEVEL_MAX = 12;
export const UI_ZOOM_LEVEL_DEFAULT = 0;

/** 限制 zoom level 在合法区间 */
export function clampUiZoomLevel(level: number): number {
  return Math.min(UI_ZOOM_LEVEL_MAX, Math.max(UI_ZOOM_LEVEL_MIN, Math.round(level)));
}

/** level → 实际缩放比例 */
export function scaleFromLevel(level: number): number {
  const clamped = clampUiZoomLevel(level);
  return Math.round(Math.pow(UI_ZOOM_FACTOR, clamped) * 100) / 100;
}

/** 比例 → 最近 zoom level（迁移旧配置用） */
export function levelFromScale(scale: number): number {
  if (!Number.isFinite(scale) || scale <= 0) return UI_ZOOM_LEVEL_DEFAULT;
  return clampUiZoomLevel(Math.round(Math.log(scale) / Math.log(UI_ZOOM_FACTOR)));
}

/** 读取持久化的 zoom level（含旧版 ds_ui_zoom 迁移） */
export function loadUiZoomLevel(): number {
  try {
    const raw = localStorage.getItem(UI_ZOOM_LEVEL_STORAGE_KEY);
    if (raw !== null) {
      const n = Number(raw);
      if (Number.isFinite(n)) return clampUiZoomLevel(n);
    }
    const legacy = localStorage.getItem(UI_ZOOM_LEGACY_SCALE_KEY);
    if (legacy !== null) {
      const scale = Number(legacy);
      if (Number.isFinite(scale)) {
        const level = levelFromScale(scale);
        saveUiZoomLevel(level);
        return level;
      }
    }
  } catch {
    /* 忽略 */
  }
  return UI_ZOOM_LEVEL_DEFAULT;
}

/** 持久化 zoom level */
export function saveUiZoomLevel(level: number): void {
  localStorage.setItem(UI_ZOOM_LEVEL_STORAGE_KEY, String(clampUiZoomLevel(level)));
}

/** 浏览器调试：同步应用 CSS zoom（启动首帧前调用，避免闪烁） */
export function applyUiZoomSync(scale: number): void {
  if (isTauri()) return;
  document.documentElement.style.setProperty("zoom", String(scale));
}

/** 将缩放应用到 WebView（Tauri）或 document（浏览器调试） */
export async function applyUiZoom(scale: number): Promise<number> {
  const next = Math.round(scale * 100) / 100;
  if (isTauri()) {
    await getCurrentWebview().setZoom(next);
  } else {
    document.documentElement.style.setProperty("zoom", String(next));
  }
  return next;
}

/** 按 level 应用缩放并持久化 */
export async function applyUiZoomLevel(level: number): Promise<number> {
  const clamped = clampUiZoomLevel(level);
  const scale = scaleFromLevel(clamped);
  await applyUiZoom(scale);
  saveUiZoomLevel(clamped);
  return scale;
}

/** 状态栏展示用百分比文案 */
export function formatUiZoomLabel(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}
