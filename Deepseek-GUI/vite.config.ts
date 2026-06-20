import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Vite 配置：固定开发端口为 1420，与 Tauri 壳和后端 CORS 白名单保持一致
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    target: "es2021",
  },
});
