/**
 * Playwright 全局收尾: 杀 Web 服务器进程, 释放 8080 端口。
 *
 * 先杀 can-monitor 子进程 (精确 PID), 其退出后 pty 主端读到 EIO,
 * start-server.py 的排空循环自然结束; 再杀 python 包装进程兜底。
 */
import { existsSync, readFileSync, rmSync } from "node:fs";

const PIDFILES: [string, "SIGTERM" | "SIGKILL"][] = [
  ["/tmp/canmonitor-e2e.pid", "SIGTERM"],
  ["/tmp/canmonitor-e2e.python.pid", "SIGTERM"],
];

export default async function globalTeardown(): Promise<void> {
  for (const [file, signal] of PIDFILES) {
    if (!existsSync(file)) continue;
    try {
      const pid = Number(readFileSync(file, "utf8").trim());
      if (Number.isInteger(pid) && pid > 0) {
        process.kill(pid, signal);
        console.log(`[e2e] 已发送 ${signal} 到 pid=${pid} (${file})`);
      }
    } catch (e) {
      // ESRCH: 进程已退出, 属正常; 其他错误仅告警。
      console.warn(`[e2e] 清理 ${file} 失败: ${String(e).slice(0, 200)}`);
    }
    try {
      rmSync(file);
    } catch {
      /* 忽略 */
    }
  }

  // 等待进程完全退出, 端口释放后再结束。
  await new Promise((r) => setTimeout(r, 800));
}
