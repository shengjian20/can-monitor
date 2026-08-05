import type { FilterState } from "./FrameTable";
import "./FilterPanel.css";

interface Props {
  /** 当前过滤状态。 */
  filter: FilterState;
  /** 过滤状态变更回调。 */
  onChange: (filter: FilterState) => void;
  /** 清空帧列表回调 (可选, 不传则不显示清空按钮)。 */
  onClear?: () => void;
}

/** 协议选项值 → 显示文本映射。 */
const PROTOCOL_OPTIONS: { value: FilterState["protocol"]; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "canopen", label: "CANopen" },
  { value: "j1939", label: "J1939" },
  { value: "raw", label: "原始" },
];

/** 方向选项值 → 显示文本映射。 */
const DIR_OPTIONS: { value: FilterState["dir"]; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "rx", label: "RX" },
  { value: "tx", label: "TX" },
];

/**
 * 帧流过滤面板。
 *
 * 提供协议下拉选择、方向三态切换、清空按钮。
 * FilterState 类型从 FrameTable.tsx 导入。
 */
export function FilterPanel({ filter, onChange, onClear }: Props) {
  const handleProtocolChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    onChange({ ...filter, protocol: e.target.value as FilterState["protocol"] });
  };

  const handleDirChange = (dir: FilterState["dir"]) => {
    onChange({ ...filter, dir });
  };

  return (
    <div className="filter-panel">
      {/* 协议下拉 */}
      <label>
        协议
        <select value={filter.protocol} onChange={handleProtocolChange}>
          {PROTOCOL_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      {/* 方向切换按钮组 */}
      <label>方向</label>
      <div className="dir-group">
        {DIR_OPTIONS.map((opt) => (
          <button
            key={opt.value}
            className={`dir-btn ${filter.dir === opt.value ? "active" : ""}`}
            onClick={() => handleDirChange(opt.value)}
            type="button"
          >
            {opt.label}
          </button>
        ))}
      </div>

      {/* 清空按钮 */}
      {onClear && (
        <button className="filter-clear" onClick={onClear} type="button">
          清空
        </button>
      )}
    </div>
  );
}
