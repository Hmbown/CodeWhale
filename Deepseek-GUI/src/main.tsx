import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles.css";
import { applyUiZoomSync, loadUiZoomLevel, scaleFromLevel } from "./utils/uiZoom";

// 浏览器调试：首帧前同步恢复缩放，避免 100% 闪烁
applyUiZoomSync(scaleFromLevel(loadUiZoomLevel()));

// 应用入口：挂载 React 根组件
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
