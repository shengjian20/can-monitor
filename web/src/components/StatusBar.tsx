import { useState, useEffect, useCallback } from "react";
import type { Api, Status } from "../types";

interface Props {
  api: Api;
  running: boolean;
}

/**
 * 状态栏组件 —— 轮询 api.getStatus (运行时每 1s, 停止时停止)。
 *
 * 显示: 运行状态灯 (绿/红), 帧总数, CANopen/J1939 计数, 错误数, 丢弃数。
 * 轮询在组件内 useEffect 管理, cleanup 时清除定时器。
 */
export function StatusBar({ api, running }: Props) {
  const [status, setStatus] = useState<Status | null>(null);

  // ── 状态轮询 (仅 running 时) ──────────────────────────────────

  const pollStatus = useCallback(async () => {
    try {
      const s = await api.getStatus();
      setStatus(s);
    } catch {
      // 静默: 服务可能未启动。
    }
  }, [api]);

  useEffect(() => {
    if (!running) {
      // 停止时刷新一次最终状态。
      pollStatus();
      return;
    }

    // 运行中: 立即轮询一次 + 每 1s 轮询。
    pollStatus();
    const timer = setInterval(pollStatus, 1000);

    return () => {
      clearInterval(timer);
    };
  }, [running, pollStatus]);

  // ── 渲染 ──────────────────────────────────────────────────────

  const isRunning = status?.running ?? false;

  return (
    <div className="status-bar">
      {/* 运行状态灯 */}
      <span className={`status-indicator ${isRunning ? "status-on" : "status-off"}`}>
        <span className={`status-dot ${isRunning ? "dot-green" : "dot-red"}`} />
        {isRunning ? "运行中" : "已停止"}
      </span>

      {/* 分隔线 */}
      <span className="status-divider" />

      {/* 帧统计 */}
      <span className="status-count">
        帧: <strong>{status?.total ?? 0}</strong>
      </span>

      {/* 协议统计 */}
      <span className="status-proto">
        CANopen: <strong>{status?.canopen ?? 0}</strong>
      </span>
      <span className="status-proto">
        J1939: <strong>{status?.j1939 ?? 0}</strong>
      </span>

      {/* 错误与丢弃 */}
      <span className={`status-warn ${status?.error ? "has-warn" : ""}`}>
        错误: <strong>{status?.error ?? 0}</strong>
      </span>
      <span className={`status-warn ${status?.dropped ? "has-warn" : ""}`}>
        丢弃: <strong>{status?.dropped ?? 0}</strong>
      </span>
    </div>
  );
}
