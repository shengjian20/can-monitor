import { useState, useEffect, useCallback, useRef } from "react";
import type { DeviceInfo, FrameData, Status } from "./types";
import { createApi } from "./api";
import { DeviceSelect } from "./components/DeviceSelect";
import { FrameTable } from "./components/FrameTable";
import { StatusBar } from "./components/StatusBar";
import "./App.css";

// 帧缓冲上限 (保留最近 N 帧, 避免内存无限增长)。
const MAX_FRAMES = 1000;

// 全局 Api 实例 (模块级单例, Tauri / 浏览器各一个)。
const api = createApi();

function App() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [selectedDevice, setSelectedDevice] = useState("");
  const [running, setRunning] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [frames, setFrames] = useState<FrameData[]>([]);
  const [error, setError] = useState<string | null>(null);

  // 用于 cleanup 的 ref。
  const unsubRef = useRef<(() => void) | null>(null);
  const statusTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // ── 加载设备列表 ──────────────────────────────────────────────
  const refreshDevices = useCallback(async () => {
    try {
      const list = await api.listDevices();
      setDevices(list);
      setError(null);
    } catch (e) {
      setError(`加载设备失败: ${e instanceof Error ? e.message : e}`);
    }
  }, []);

  useEffect(() => {
    refreshDevices();
  }, [refreshDevices]);

  // ── 状态轮询 ─────────────────────────────────────────────────
  const pollStatus = useCallback(async () => {
    try {
      const s = await api.getStatus();
      setStatus(s);
      setRunning(s.running);
    } catch {
      // 静默: 服务可能未启动。
    }
  }, []);

  // 运行中时轮询状态。
  useEffect(() => {
    if (running) {
      pollStatus();
      statusTimerRef.current = setInterval(pollStatus, 1000);
    }
    return () => {
      if (statusTimerRef.current) {
        clearInterval(statusTimerRef.current);
        statusTimerRef.current = null;
      }
    };
  }, [running, pollStatus]);

  // ── 帧订阅 ──────────────────────────────────────────────────
  const startSubscription = useCallback(() => {
    // 清理旧订阅。
    if (unsubRef.current) {
      unsubRef.current();
      unsubRef.current = null;
    }

    unsubRef.current = api.subscribeFrames((frame) => {
      setFrames((prev) => {
        const next = [...prev, frame];
        // 超过上限时截断旧帧。
        if (next.length > MAX_FRAMES) {
          return next.slice(next.length - MAX_FRAMES);
        }
        return next;
      });
    });
  }, []);

  // ── 开始监控 ─────────────────────────────────────────────────
  const handleStart = useCallback(async () => {
    if (!selectedDevice) {
      setError("请先选择设备");
      return;
    }
    try {
      setError(null);
      setFrames([]);
      await api.startMonitor(selectedDevice);
      setRunning(true);
      startSubscription();
    } catch (e) {
      setError(`启动监控失败: ${e instanceof Error ? e.message : e}`);
    }
  }, [selectedDevice, startSubscription]);

  // ── 停止监控 ─────────────────────────────────────────────────
  const handleStop = useCallback(async () => {
    try {
      setError(null);
      if (unsubRef.current) {
        unsubRef.current();
        unsubRef.current = null;
      }
      await api.stopMonitor();
      setRunning(false);
      await pollStatus();
    } catch (e) {
      setError(`停止监控失败: ${e instanceof Error ? e.message : e}`);
    }
  }, [pollStatus]);

  // ── 组件卸载时清理 ───────────────────────────────────────────
  useEffect(() => {
    return () => {
      if (unsubRef.current) unsubRef.current();
      if (statusTimerRef.current) clearInterval(statusTimerRef.current);
    };
  }, []);

  return (
    <div className="app">
      {/* 顶栏: 标题 + 设备选择 + 控制按钮 */}
      <header className="app-header">
        <h1 className="app-title">CAN Monitor</h1>
        <div className="app-controls">
          <DeviceSelect
            devices={devices}
            selected={selectedDevice}
            onChange={setSelectedDevice}
          />
          {!running ? (
            <button className="btn btn-start" onClick={handleStart}>
              开始监控
            </button>
          ) : (
            <button className="btn btn-stop" onClick={handleStop}>
              停止监控
            </button>
          )}
          <button
            className="btn btn-refresh"
            onClick={refreshDevices}
            disabled={running}
          >
            刷新设备
          </button>
        </div>
      </header>

      {/* 错误提示 */}
      {error && <div className="app-error">{error}</div>}

      {/* 主区: 帧表格 */}
      <main className="app-main">
        <FrameTable frames={frames} />
      </main>

      {/* 状态栏 */}
      <footer className="app-footer">
        <StatusBar status={status} />
      </footer>
    </div>
  );
}

export default App;
