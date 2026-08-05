import { useState, useEffect, useCallback, useRef } from "react";
import type { DeviceInfo, FrameData } from "./types";
import type { FilterState } from "./components/FrameTable";
import { createApi } from "./api";
import { isTauri } from "./env";
import { DeviceSelect } from "./components/DeviceSelect";
import { SendPanel } from "./components/SendPanel";
import { StatusBar } from "./components/StatusBar";
import "./App.css";

// T19 组件: 若此刻文件尚缺导出 (并行竞态), 构建会失败; 等 T19 完成后即成功。
import { FrameTable, FilterPanel, filterFrames } from "./components/FrameTable";

const MAX_FRAMES = 1000;

const api = createApi();

function App() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [selectedDevice, setSelectedDevice] = useState("");
  const [running, setRunning] = useState(false);
  const [frames, setFrames] = useState<FrameData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<FilterState>({
    protocol: "all",
    dir: "all",
  });

  const unsubRef = useRef<(() => void) | null>(null);

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

  // ── 帧订阅 ──────────────────────────────────────────────────
  const startSubscription = useCallback(() => {
    if (unsubRef.current) {
      unsubRef.current();
      unsubRef.current = null;
    }

    unsubRef.current = api.subscribeFrames((frame) => {
      setFrames((prev) => {
        const next = [...prev, frame];
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
    } catch (e) {
      setError(`停止监控失败: ${e instanceof Error ? e.message : e}`);
    }
  }, []);

  // ── 组件卸载时清理 ───────────────────────────────────────────
  useEffect(() => {
    return () => {
      if (unsubRef.current) unsubRef.current();
    };
  }, []);

  // ── 过滤后帧 (T19 filterFrames) ────────────────────────────
  const filteredFrames = filterFrames(frames, filter);

  return (
    <div className="app">
      {/* 顶栏 */}
      <header className="app-header">
        <h1 className="app-title">CAN Monitor</h1>
        <div className="app-controls">
          <DeviceSelect
            devices={devices}
            selected={selectedDevice}
            onChange={setSelectedDevice}
          />
          {!running ? (
            <button
              className="btn btn-start"
              onClick={handleStart}
              disabled={!selectedDevice}
            >
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
            刷新
          </button>
        </div>
      </header>

      {/* 错误提示 */}
      {error && <div className="app-error">{error}</div>}

      {/* 主区: 过滤面板 + 帧表格 */}
      <main className="app-main">
        <FilterPanel filter={filter} onChange={setFilter} onClear={() => setFrames([])} />
        <FrameTable frames={filteredFrames} />
      </main>

      {/* 底栏: 状态栏 + 发送面板 + 模式标识 */}
      <div className="app-bottom">
        <SendPanel api={api} disabled={!running} />
        <footer className="app-footer">
          <StatusBar api={api} running={running} />
          <span className="app-mode">{isTauri() ? "Tauri" : "浏览器"}</span>
        </footer>
      </div>
    </div>
  );
}

export default App;
