/// <reference types="vite/client" />

// Tauri v2 全局类型声明 (window.__TAURI__ / __TAURI_INTERNALS__)。
interface TauriWindow {
  __TAURI__?: Record<string, unknown>;
  __TAURI_INTERNALS__?: Record<string, unknown>;
}

declare global {
  interface Window extends TauriWindow {}
}
