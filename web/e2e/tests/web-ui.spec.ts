/**
 * Web UI e2e —— 浏览器模式下与真实 Rust 服务器交互。
 *
 * 前置: globalSetup 已通过 pty 启动
 *   target/debug/can-monitor --backend none --web-write --web-port 127.0.0.1:8080
 * 并探活成功。本文件所有用例串行 (单 worker), 共享同一服务器实例。
 *
 * 覆盖 (任务 T22):
 *  1. 页面加载 (标题/三区布局/浏览器模式标识)
 *  2. 设备下拉 (空态文案 + 手动输入接口名)
 *  3. 启动监控 (running=true, 状态灯/文案)
 *  4. 停止监控 (running=false)
 *  5. 发送面板校验 (非法 ID/data → 中文错误; 合法 → 无报错)
 *  6. 过滤面板 (协议/方向控件可交互)
 *  7. console 无 error (WS 连接失败除外)
 *
 * 选择器策略: 优先中文文案 (text=/placeholder), 少用 CSS 类名 (会变)。
 * 未修改任何 App.tsx 生产代码 (业务逻辑零改动)。
 */
import { test, expect, type Page } from "@playwright/test";
import path from "node:path";
import { fileURLToPath } from "node:url";

// 证据目录 (gitignore 已排除 .omo/): 关键状态截图。
const EVIDENCE_DIR = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../.omo/evidence"
);

// ── console 错误收集 (串行运行, 模块级数组安全) ──────────────────
// 记录所有 console error + pageerror; 测试 7 断言非 WS 失败错误为 0。
const consoleErrors: string[] = [];

test.beforeEach(async ({ page }) => {
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(`[console] ${msg.text()}`);
  });
  page.on("pageerror", (err) => consoleErrors.push(`[pageerror] ${err.message}`));
  await page.goto("/");
});

// ── 交互辅助函数 ────────────────────────────────────────────────

/** 通过下拉"手动输入接口名..."进入手动模式, 填入设备标识并回车确认。 */
async function setDeviceManually(page: Page, deviceId: string): Promise<void> {
  await page.locator("select.device-select").selectOption("__manual__");
  const input = page.getByPlaceholder("接口名 (如 can0)");
  await expect(input).toBeVisible();
  await input.fill(deviceId);
  await input.press("Enter");
  // 手动模式退出, 回到下拉形态。
  await expect(input).toHaveCount(0);
}

/** 确保监控处于运行态 (若已运行则跳过, 否则手动选择设备并点击开始监控)。 */
async function ensureRunning(page: Page): Promise<void> {
  const stopBtn = page.getByRole("button", { name: "停止监控" });
  if (await stopBtn.isVisible()) return;
  await setDeviceManually(page, "none");
  await page.getByRole("button", { name: "开始监控" }).click();
  await expect(page.getByText("运行中")).toBeVisible();
}

test.describe.configure({ mode: "serial" });

// ── 1. 页面加载 ─────────────────────────────────────────────────

test("1. 页面加载: 标题/三区布局/浏览器模式标识", async ({ page }) => {
  // 标题 (index.html <title>)。
  await expect(page).toHaveTitle("CAN Monitor");

  // 顶栏主标题。
  await expect(
    page.getByRole("heading", { name: "CAN Monitor" })
  ).toBeVisible();

  // 三区布局: 顶栏 / 主区 / 底栏。
  await expect(page.locator(".app-header")).toBeVisible();
  await expect(page.locator(".app-main")).toBeVisible();
  await expect(page.locator(".app-bottom")).toBeVisible();

  // 模式标识: 浏览器 (非 Tauri 环境)。
  await expect(page.locator(".app-mode")).toHaveText("浏览器");

  await page.screenshot({ path: `${EVIDENCE_DIR}/v2-task-22-load.png`, fullPage: true });
});

// ── 2. 设备下拉 ─────────────────────────────────────────────────

test("2. 设备下拉: 存在/空态/手动输入接口名", async ({ page }) => {
  const select = page.locator("select.device-select");
  await expect(select).toBeVisible();

  // 空态: 服务器无可用设备时显示中文提示 (--backend none 无 CAN 接口)。
  await expect(page.getByText("无可用设备")).toBeVisible();

  // 下拉占位选项。
  await expect(select.locator("option", { hasText: "选择设备" })).toHaveCount(1);

  // 手动输入接口名: 选择 "__manual__" 后出现输入框。
  await setDeviceManually(page, "can0");

  // 确认后回到下拉形态, 且"开始监控"按钮因已选设备而可用。
  await expect(select).toBeVisible();
  await expect(page.getByRole("button", { name: "开始监控" })).toBeEnabled();
});

// ── 3. 启动监控 ─────────────────────────────────────────────────

test("3. 启动监控: 状态灯/文案变 running", async ({ page }) => {
  // 初始应为"已停止"。
  await expect(page.getByText("已停止")).toBeVisible();

  // 选择后端 none (服务器接受 device_id "none") 并开始监控。
  await setDeviceManually(page, "none");
  await page.getByRole("button", { name: "开始监控" }).click();

  // 按钮切换为"停止监控", 状态栏轮询到 running=true 显示"运行中"。
  await expect(page.getByRole("button", { name: "停止监控" })).toBeVisible();
  await expect(page.getByText("运行中")).toBeVisible();

  // 状态灯绿 (status-on 类) — 与 T16 curl 实测 (running=true) 一致。
  await expect(page.locator(".status-indicator.status-on")).toBeVisible();

  await page.screenshot({ path: `${EVIDENCE_DIR}/v2-task-22-running.png`, fullPage: true });
});

// ── 4. 停止监控 ─────────────────────────────────────────────────

test("4. 停止监控: running=false", async ({ page }) => {
  await ensureRunning(page);

  await page.getByRole("button", { name: "停止监控" }).click();

  // 回到"开始监控"按钮, 状态栏轮询到 running=false 显示"已停止"。
  await expect(page.getByRole("button", { name: "开始监控" })).toBeVisible();
  await expect(page.getByText("已停止")).toBeVisible();
  await expect(page.locator(".status-indicator.status-off")).toBeVisible();
});

// ── 5. 发送面板校验 ─────────────────────────────────────────────

test("5. 发送面板: 非法 ID/非法 data 中文报错, 合法输入无报错", async ({ page }) => {
  // 发送面板在未运行时为禁用态, 先确保运行。
  await ensureRunning(page);

  const idInput = page.getByPlaceholder("0x181");
  const dataInput = page.getByPlaceholder("01 02 03 04");
  const sendBtn = page.getByRole("button", { name: "发送" });
  const sendError = page.locator(".send-error");

  // 非法 ID ("XYZ") → 中文错误提示, 不发请求。
  await idInput.fill("XYZ");
  await dataInput.fill("01");
  await sendBtn.click();
  await expect(sendError).toHaveText(/ID 格式错误/);

  // 非法 data ("GG") → 中文错误提示。
  await idInput.fill("0x181");
  await dataInput.fill("GG");
  await sendBtn.click();
  await expect(sendError).toHaveText(/数据格式错误/);

  // 合法输入 (0x181 + 01 02 FF) → 无报错; 发送成功清空 data 保留 ID。
  // (send 行为以服务器实际响应为准: --web-write 下应返回 ok, 不断言帧数)
  await dataInput.fill("01 02 FF");
  await sendBtn.click();
  await expect(sendError).toHaveCount(0);
  await expect(dataInput).toHaveValue("");
  await expect(idInput).toHaveValue("0x181");

  await page.screenshot({ path: `${EVIDENCE_DIR}/v2-task-22-send-invalid.png`, fullPage: true });
});

// ── 6. 过滤面板 ─────────────────────────────────────────────────

test("6. 过滤面板: 协议/方向控件可交互不崩溃", async ({ page }) => {
  // 协议下拉切换到 CANopen。
  const protocolSelect = page.locator(".filter-panel select");
  await expect(protocolSelect).toBeVisible();
  await protocolSelect.selectOption("canopen");
  await expect(protocolSelect).toHaveValue("canopen");

  // 方向按钮: RX / TX / 全部 三态切换。
  const rxBtn = page.locator(".filter-panel .dir-btn", { hasText: "RX" });
  const txBtn = page.locator(".filter-panel .dir-btn", { hasText: "TX" });
  const allBtn = page.locator(".filter-panel .dir-btn", { hasText: "全部" });
  await rxBtn.click();
  await expect(rxBtn).toHaveClass(/active/);
  await txBtn.click();
  await expect(txBtn).toHaveClass(/active/);
  await allBtn.click();
  await expect(allBtn).toHaveClass(/active/);

  // 清空按钮可点击。
  await page.getByRole("button", { name: "清空" }).click();

  // 切换协议回全部, 收尾。
  await protocolSelect.selectOption("all");
});

// ── 7. console 无 error ─────────────────────────────────────────

test("7. console 无 error (WS 连接失败除外)", async () => {
  // 服务器常驻运行, WS /ws 正常连接并收 [] 心跳 → 不应出现任何 console error。
  // 若 WS 因网络瞬断失败, 其 error 属可豁免项, 从断言集合中剔除。
  const unrelated = consoleErrors.filter(
    (e) =>
      !/WebSocket.*(failed|connection error)/i.test(e) &&
      !/WebSocket is closed before the connection is established/i.test(e)
  );
  expect(unrelated, `存在非 WS 的 console error:\n${unrelated.join("\n")}`).toHaveLength(0);
});
