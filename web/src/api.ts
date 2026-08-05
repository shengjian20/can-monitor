/**
 * 统一 API 层 —— 按运行环境 (Tauri / 浏览器) 返回不同实现。
 *
 * Tauri 模式: invoke() 调 T17 命令 + Channel 帧流。
 * 浏览器模式: fetch REST (T16) + WebSocket 批量帧流。
 *
 * App 组件只使用 Api 接口, 完全不感知底层模式。
 */

import type { Api, DeviceInfo, FrameData, Status } from "./types";
import { isTauri } from "./env";

// ─── Tauri 实现 ─────────────────────────────────────────────────

/** Tauri 模式: 通过 @tauri-apps/api/core 的 invoke + Channel 通信。 */
class TauriApi implements Api {
  // 动态导入, 仅在 Tauri 运行时构造此类。
  // 静态 import 在纯浏览器 build 不会报错 (npm 包存在),
  // 但浏览器运行时调用 invoke 会失败, 所以必须隔离。
  private invoke: typeof import("@tauri-apps/api/core").invoke | null = null;
  private Channel:
    | typeof import("@tauri-apps/api/core").Channel
    | null = null;
  private initPromise: Promise<void>;

  constructor() {
    // 动态导入 @tauri-apps/api/core, 仅在 Tauri 环境执行。
    this.initPromise = import("@tauri-apps/api/core").then((mod) => {
      this.invoke = mod.invoke;
      this.Channel = mod.Channel;
    });
  }

  private async ready() {
    await this.initPromise;
    if (!this.invoke || !this.Channel) {
      throw new Error("Tauri API 初始化失败");
    }
  }

  async listDevices(): Promise<DeviceInfo[]> {
    await this.ready();
    return this.invoke!("list_devices");
  }

  async startMonitor(deviceId: string): Promise<void> {
    await this.ready();
    return this.invoke!("start_monitor", { deviceId });
  }

  async stopMonitor(): Promise<void> {
    await this.ready();
    return this.invoke!("stop_monitor");
  }

  async sendFrame(f: {
    id: string;
    ext: boolean;
    data: string;
  }): Promise<void> {
    await this.ready();
    return this.invoke!("send_frame", { frame: f });
  }

  async getStatus(): Promise<Status> {
    await this.ready();
    return this.invoke!("get_status");
  }

  subscribeFrames(onFrame: (f: FrameData) => void): () => void {
    // subscribe_frames 需要同步准备 Channel, 但 invoke 是异步。
    // 用闭包捕获 channel_id, cleanup 时调 unsubscribe_frames。
    let channelId: number | null = null;
    let disposed = false;

    const setup = async () => {
      await this.ready();
      if (disposed) return;

      // Channel: Tauri v2 IPC 双向通道, onmessage 接收后端推送。
      const ch = new this.Channel!<FrameData>();
      ch.onmessage = (frame: FrameData) => {
        onFrame(frame);
      };

      channelId = await this.invoke!("subscribe_frames", {
        onFrame: ch,
      });
    };

    setup();

    // 返回 cleanup 函数。
    return () => {
      disposed = true;
      if (channelId !== null && this.invoke) {
        this.invoke("unsubscribe_frames", { channelId }).catch(() => {
          // 静默: 连接已断或 Tauri 已退出。
        });
      }
    };
  }
}

// ─── 浏览器 (HTTP + WebSocket) 实现 ─────────────────────────────

/** 浏览器模式: REST (T16) + WebSocket 批量帧流。 */
class HttpApi implements Api {
  private baseUrl: string;
  private wsUrl: string;

  constructor() {
    // 用 location.hostname 拼接, 方便局域网调试但默认本机。
    const host = location.hostname || "127.0.0.1";
    this.baseUrl = `http://${host}:8080/api`;
    this.wsUrl = `ws://${host}:8080/ws`;
  }

  async listDevices(): Promise<DeviceInfo[]> {
    const res = await fetch(`${this.baseUrl}/devices`);
    if (!res.ok) throw new Error(`listDevices 失败: ${res.status}`);
    return res.json();
  }

  async startMonitor(deviceId: string): Promise<void> {
    const res = await fetch(`${this.baseUrl}/monitor/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ device_id: deviceId }),
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(`startMonitor 失败: ${res.status} ${text}`);
    }
  }

  async stopMonitor(): Promise<void> {
    const res = await fetch(`${this.baseUrl}/monitor/stop`, {
      method: "POST",
    });
    if (!res.ok) throw new Error(`stopMonitor 失败: ${res.status}`);
  }

  async sendFrame(f: {
    id: string;
    ext: boolean;
    data: string;
  }): Promise<void> {
    const res = await fetch(`${this.baseUrl}/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(f),
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(`sendFrame 失败: ${res.status} ${text}`);
    }
  }

  async getStatus(): Promise<Status> {
    const res = await fetch(`${this.baseUrl}/status`);
    if (!res.ok) throw new Error(`getStatus 失败: ${res.status}`);
    return res.json();
  }

  subscribeFrames(onFrame: (f: FrameData) => void): () => void {
    // WebSocket 连接: 服务器每 30ms 或 50 帧发一个 JSON 数组。
    const ws = new WebSocket(this.wsUrl);

    ws.onmessage = (ev) => {
      try {
        const batch: FrameData[] = JSON.parse(ev.data);
        // 批量帧逐帧回调 (空数组 [] 是心跳, 自然跳过)。
        for (const frame of batch) {
          onFrame(frame);
        }
      } catch {
        // 解析失败静默丢弃。
      }
    };

    ws.onerror = () => {
      // 错误由 onclose 后续处理。
    };

    // 返回 cleanup: 关闭 WebSocket。
    return () => {
      ws.close();
    };
  }
}

// ─── 工厂函数 ────────────────────────────────────────────────────

/**
 * 按运行环境创建 Api 实例。
 * - Tauri 模式: invoke + Channel
 * - 浏览器模式: fetch + WebSocket
 */
export function createApi(): Api {
  if (isTauri()) {
    console.log("[api] Tauri 模式: invoke + Channel");
    return new TauriApi();
  }
  console.log("[api] 浏览器模式: REST + WebSocket");
  return new HttpApi();
}
