/**
 * Playwright e2e 配置 —— Web UI 与真实 Rust 服务器 (REST + WS + 静态) 的端到端测试。
 *
 * - baseURL: 服务器监听 127.0.0.1:8080 (T16 默认端口)。
 * - 单 worker + 串行: 所有用例共享一个服务器实例, 状态在用例间延续。
 * - globalSetup: pty 启动服务器并探活; globalTeardown: 杀进程释放端口。
 * - 截图: 失败自动截图 (test-results/); 关键状态显式截图存入 .omo/evidence/。
 */
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  globalSetup: "./global-setup.ts",
  globalTeardown: "./global-teardown.ts",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  // 单 worker 串行: 共享一个服务器实例, 避免并发启动多服务器/端口冲突。
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [
    ["list"],
    // HTML 报告与测试产物写入 .omo/ (已 gitignore), 避免污染 web/ 提交范围。
    ["html", { open: "never", outputFolder: "../../.omo/playwright-report" }],
  ],
  use: {
    baseURL: "http://127.0.0.1:8080",
    // 中文界面, 桌面尺寸足够容纳三区布局。
    viewport: { width: 1280, height: 800 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  outputDir: "../../.omo/playwright-results",
});
