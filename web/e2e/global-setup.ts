/**
 * Playwright 全局前置: 通过 pty 启动 Rust Web 服务器 (真实 e2e, 不 mock)。
 *
 * 服务器 = target/debug/can-monitor --backend none --web-write --web-port 127.0.0.1:8080。
 * 非 TTY 下 ratatui 会 panic (ENOTTY), 必须用 python3 pty 包装 (web/e2e/start-server.py)。
 * 启动后轮询 GET /api/status 直到可连, 才放行测试。
 */
import { spawn } from "node:child_process";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const LAUNCHER = path.join(ROOT, "web/e2e/start-server.py");
const PIDFILE = "/tmp/canmonitor-e2e.pid";
const PYTHON_PIDFILE = "/tmp/canmonitor-e2e.python.pid";
const BASE = "http://127.0.0.1:8080";
const READY_TIMEOUT_MS = 20_000;

export default async function globalSetup(): Promise<void> {
  // 清理上次残留的 pidfile (端口被占时启动会失败, 提前探活兜底)。
  for (const f of [PIDFILE, PYTHON_PIDFILE]) {
    try {
      rmSync(f);
    } catch {
      /* 不存在则忽略 */
    }
  }

  // 兜底: 若 8080 已被上一次运行的残留进程占用, 按 pidfile 先杀掉。
  await killIfPidFileExists(PIDFILE, "SIGTERM");
  await killIfPidFileExists(PYTHON_PIDFILE, "SIGTERM");

  const py = spawn("python3", [LAUNCHER], {
    cwd: ROOT, // 服务器 ServeDir 指向 "web/dist", 须以仓库根为工作目录
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  writeFileSync(PYTHON_PIDFILE, String(py.pid ?? ""));
  py.stdout?.on("data", () => {
    /* 排空, 防止阻塞 */
  });
  py.stderr?.on("data", () => {
    /* 排空 */
  });
  py.on("exit", (code) => {
    if (code !== 0 && !existsSync(PIDFILE)) {
      console.error(`[e2e] start-server.py 异常退出 code=${code}`);
    }
  });

  // 探活: 等待 /api/status 可连 (同时等待 pidfile 写入)。
  const deadline = Date.now() + READY_TIMEOUT_MS;
  let lastBody = "";
  while (Date.now() < deadline) {
    if (existsSync(PIDFILE)) {
      try {
        const res = await fetch(`${BASE}/api/status`, {
          signal: AbortSignal.timeout(1000),
        });
        if (res.ok) {
          lastBody = await res.text();
          console.log(`[e2e] 服务器就绪 (pid=${readFileSync(PIDFILE, "utf8").trim()}): ${lastBody}`);
          return;
        }
      } catch {
        /* 未就绪, 继续等待 */
      }
    }
    await new Promise((r) => setTimeout(r, 400));
  }

  throw new Error(
    `[e2e] 服务器 ${BASE} ${READY_TIMEOUT_MS / 1000}s 内未就绪 (pidfile 存在=${existsSync(PIDFILE)}, 最后响应=${lastBody})`
  );
}

/** 若 pidfile 存在且 PID 有效, 发送信号 (用于清理残留进程)。 */
async function killIfPidFileExists(file: string, signal: "SIGTERM"): Promise<void> {
  if (!existsSync(file)) return;
  try {
    const pid = Number(readFileSync(file, "utf8").trim());
    if (Number.isInteger(pid) && pid > 0) process.kill(pid, signal);
  } catch (e) {
    console.warn(`[e2e] 清理残留进程 ${file} 失败: ${e}`);
  }
}
