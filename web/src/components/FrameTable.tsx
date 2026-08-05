import type { FrameData } from "../types";

interface Props {
  frames: FrameData[];
}

/** 帧表格占位 — T19 会替换为完整虚拟滚动实现。 */
export function FrameTable({ frames }: Props) {
  if (frames.length === 0) {
    return (
      <div className="frame-table-empty">等待帧数据...</div>
    );
  }

  return (
    <div className="frame-table-wrap">
      <table className="frame-table">
        <thead>
          <tr>
            <th>时间</th>
            <th>ID</th>
            <th>方向</th>
            <th>数据</th>
            <th>协议</th>
            <th>摘要</th>
          </tr>
        </thead>
        <tbody>
          {frames.map((f, i) => (
            <tr key={i} className={f.dir === "tx" ? "row-tx" : "row-rx"}>
              <td className="col-ts">{f.ts}</td>
              <td className="col-id">{f.id}</td>
              <td className="col-dir">{f.dir.toUpperCase()}</td>
              <td className="col-data">{f.data}</td>
              <td className="col-proto">{f.protocol}</td>
              <td className="col-summary">{f.summary}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
