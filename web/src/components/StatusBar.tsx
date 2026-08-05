import type { Status } from "../types";

interface Props {
  status: Status | null;
}

/** 状态栏占位 — T20 会替换为更丰富的状态显示。 */
export function StatusBar({ status }: Props) {
  if (!status) {
    return <div className="status-bar">状态: 未连接</div>;
  }

  return (
    <div className="status-bar">
      <span className={status.running ? "status-on" : "status-off"}>
        {status.running ? "● 监控中" : "○ 已停止"}
      </span>
      <span className="status-count">帧计数: {status.total}</span>
      <span className="status-proto">CANopen: {status.canopen}</span>
      <span className="status-proto">J1939: {status.j1939}</span>
      <span className="status-proto">错误: {status.error}</span>
      <span className="status-proto">丢弃: {status.dropped}</span>
    </div>
  );
}
