import type { DeviceInfo } from "../types";

interface Props {
  devices: DeviceInfo[];
  selected: string;
  onChange: (id: string) => void;
}

/** 设备选择下拉 — T19/T20 会替换为更完整的实现。 */
export function DeviceSelect({ devices, selected, onChange }: Props) {
  return (
    <select
      className="device-select"
      value={selected}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="">-- 选择设备 --</option>
      {devices.map((d) => (
        <option key={d.id} value={d.id} disabled={!d.available}>
          {d.name}
          {d.available ? "" : " (离线)"}
        </option>
      ))}
    </select>
  );
}
