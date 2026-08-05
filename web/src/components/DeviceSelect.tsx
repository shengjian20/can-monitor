import { useState, useRef } from "react";
import type { DeviceInfo } from "../types";

interface Props {
  devices: DeviceInfo[];
  selected: string;
  onChange: (id: string) => void;
}

/**
 * 设备选择组件 —— 下拉选择 + 手动输入 (SocketCAN 接口名)。
 *
 * 设备列表来自 api.listDevices(), 显示 id/name/model。
 * 无可用设备时提供手动输入框 (如 "can0"), Enter 生效。
 */
export function DeviceSelect({ devices, selected, onChange }: Props) {
  const [manualMode, setManualMode] = useState(false);
  const [manualValue, setManualValue] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // 可用设备 (available=true 优先)。
  const availableDevices = devices.filter((d) => d.available);
  const unavailableDevices = devices.filter((d) => !d.available);

  // 切换到手动输入模式。
  const enterManualMode = () => {
    setManualMode(true);
    // 聚焦延迟到 DOM 更新后。
    setTimeout(() => inputRef.current?.focus(), 0);
  };

  // 手动输入确认 (Enter)。
  const handleManualKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      const val = manualValue.trim();
      if (val) {
        onChange(val);
        exitManualMode();
      }
    }
    if (e.key === "Escape") {
      exitManualMode();
    }
  };

  // 从手动模式返回下拉。
  const exitManualMode = () => {
    setManualMode(false);
    setManualValue("");
  };

  // 手动模式: 显示输入框。
  if (manualMode) {
    return (
      <div className="device-manual">
        <input
          ref={inputRef}
          type="text"
          className="device-manual-input"
          placeholder="接口名 (如 can0)"
          value={manualValue}
          onChange={(e) => setManualValue(e.target.value)}
          onKeyDown={handleManualKeyDown}
        />
        <button
          className="btn btn-sm"
          onClick={() => {
            const val = manualValue.trim();
            if (val) {
              onChange(val);
              exitManualMode();
            }
          }}
          title="确认"
        >
          ✓
        </button>
        <button className="btn btn-sm" onClick={exitManualMode} title="返回列表">
          ✕
        </button>
      </div>
    );
  }

  // 下拉变更 (拦截 __manual__ 入口)。
  const handleSelectChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    if (e.target.value === "__manual__") {
      enterManualMode();
      return;
    }
    onChange(e.target.value);
  };

  return (
    <div className="device-select-wrap">
      <select
        className="device-select"
        value={selected}
        onChange={handleSelectChange}
      >
        <option value="">-- 选择设备 --</option>

        {/* 可用设备 */}
        {availableDevices.length > 0 && (
          <optgroup label="可用设备">
            {availableDevices.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name} [{d.model}]
              </option>
            ))}
          </optgroup>
        )}

        {/* 不可用设备 (灰色) */}
        {unavailableDevices.length > 0 && (
          <optgroup label="离线设备">
            {unavailableDevices.map((d) => (
              <option key={d.id} value={d.id} disabled>
                {d.name} [{d.model}] (离线)
              </option>
            ))}
          </optgroup>
        )}

        {/* 手动输入入口 */}
        <option value="__manual__">📝 手动输入接口名...</option>
      </select>

      {/* 空态提示 */}
      {devices.length === 0 && (
        <span className="device-empty-hint">无可用设备</span>
      )}
    </div>
  );
}
