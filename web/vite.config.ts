import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri v2 官方模板配置:
// - port 1420 对齐 tauri.conf.json devUrl
// - strictPort 失败则退出 (不自动换端口)
// - clearScreen false 保留 Tauri 输出
// - envPrefix TAURI_ 供 Tauri 环境变量穿透
// https://v2.tauri.app/start/frontend/vite/

export default defineConfig({
  plugins: [react()],

  // 透明代理: 避免 vite 和 Tauri CLI 清屏冲突
  clearScreen: false,

  server: {
    port: 1420,
    strictPort: true,
  },

  // Tauri 期望固定端口, 不自动打开浏览器
  envPrefix: ["VITE_", "TAURI_"],

  build: {
    // Tauri 使用 ../web/dist 作为 frontendDist
    outDir: "dist",
    // Tauri 在 Windows 上不支持嵌套 public 目录中的大写扩展名
    // minify 不在 debug 构建
    target: process.env.TAURI_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
