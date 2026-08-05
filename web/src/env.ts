/**
 * 运行时环境检测。
 *
 * Tauri v2 注入 window.__TAURI__ 全局对象 (需 tauri.conf.json app.withGlobalTauri=true
 * 或使用 @tauri-apps/api 的 isTauri())。两种检测并行, 任一命中即为 Tauri 模式。
 */
export function isTauri(): boolean {
  // @tauri-apps/api/core 的 isTauri() 是最权威的判断,
  // 但需要运行时 import; 这里先用全局对象做同步检测。
  return (
    typeof window !== "undefined" &&
    ("__TAURI__" in window || "__TAURI_INTERNALS__" in window)
  );
}
