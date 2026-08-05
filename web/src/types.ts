/** CAN 设备信息 (与 T17 commands.rs DeviceInfoJson / T16 REST /api/devices 一致)。 */
export interface DeviceInfo {
  /** 设备标识, 如 "socketcan:can0", "usbvci:0"。 */
  id: string;
  /** 显示名称。 */
  name: string;
  /** 设备类型: "SocketCan" / "UsbVci" / "Other"。 */
  kind: string;
  /** 驱动名。 */
  driver: string;
  /** 型号 (详情)。 */
  model: string;
  /** 是否可用 (在线)。 */
  available: boolean;
}

/**
 * CAN 帧数据 (与 T15/T16 帧 JSON 契约一致)。
 *
 * 字段说明:
 * - ts: u64 毫秒时间戳 (字符串, 防 JS 2^53 溢出)
 * - id: 十六进制 CAN ID, 如 "0x181"
 * - ext: 是否扩展帧 (29 位)
 * - dir: "rx" / "tx"
 * - data: 大写十六进制空格分隔, 如 "01 02 03"
 * - protocol: "canopen" / "j1939" / "raw"
 * - summary: 可读摘要
 */
export interface FrameData {
  ts: string;
  id: string;
  ext: boolean;
  dir: "rx" | "tx";
  data: string;
  protocol: string;
  summary: string;
}

/** 监控状态 (与 T17 StatusJson / T16 /api/status 一致)。 */
export interface Status {
  running: boolean;
  total: number;
  canopen: number;
  j1939: number;
  error: number;
  dropped: number;
}

/**
 * 统一 API 接口 —— Tauri 模式和浏览器模式共用。
 *
 * App 组件只面向此接口, 不感知底层运行模式。
 */
export interface Api {
  listDevices(): Promise<DeviceInfo[]>;
  startMonitor(deviceId: string): Promise<void>;
  stopMonitor(): Promise<void>;
  sendFrame(f: { id: string; ext: boolean; data: string }): Promise<void>;
  getStatus(): Promise<Status>;
  /**
   * 订阅帧流。
   * @param onFrame 每收到一帧调用一次。
   * @returns 取消订阅函数。
   */
  subscribeFrames(onFrame: (f: FrameData) => void): () => void;
}
