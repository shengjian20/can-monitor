import { useRef, useState, useEffect, useCallback } from "react";
import type { FrameData } from "../types";
import "./FrameTable.css";

// 桶导出: T20 统一从 FrameTable 导入 FilterPanel。
export { FilterPanel } from "./FilterPanel";

// ── 虚拟滚动参数 ──────────────────────────────────────────────
/** 固定行高 (px), 与 CSS .frame-table td padding + font-size + border 一致。 */
const ROW_HEIGHT = 28;
/** 可视区上下各多渲染的缓冲行数。 */
const BUFFER_ROWS = 10;
/** 自动滚动判定阈值 (px): 距离底部小于此值时视为"在底部"。 */
const SCROLL_THRESHOLD = 40;

// ── 过滤类型与纯函数 (导出供 T20/T22 复用) ────────────────────

/** 帧过滤状态。 */
export interface FilterState {
  /** 协议过滤: "all" 表示不过滤。 */
  protocol: "all" | "canopen" | "j1939" | "raw";
  /** 方向过滤: "all" 表示不过滤。 */
  dir: "all" | "rx" | "tx";
}

/**
 * 纯函数: 按 FilterState 过滤帧数组。
 * 导出供外部 (App 层 / 测试) 复用。
 */
export function filterFrames(frames: FrameData[], filter: FilterState): FrameData[] {
  if (filter.protocol === "all" && filter.dir === "all") return frames;
  return frames.filter((f) => {
    if (filter.protocol !== "all" && f.protocol !== filter.protocol) return false;
    if (filter.dir !== "all" && f.dir !== filter.dir) return false;
    return true;
  });
}

// ── 协议高亮分类 ──────────────────────────────────────────────

/**
 * 根据 protocol + summary 返回高亮 CSS 类名。
 *
 * 规则 (注释说明分类依据):
 * - canopen + summary 含 "Heartbeat" 或 "心跳" → "hl-heartbeat" (绿)
 * - canopen + summary 含 "SDO"                  → "hl-sdo"       (黄)
 * - canopen + summary 含 "TPDO" 或 "RPDO" 或 "PDO" → "hl-pdo"  (蓝)
 * - canopen + summary 含 "NMT"                   → "hl-nmt"     (橙)
 * - canopen + 其他 (EMCY/SYNC/TIME 等)            → 不加类, 保持默认
 * - j1939                                        → "hl-j1939"   (青)
 * - raw                                          → "hl-raw"     (灰)
 */
function highlightClass(f: FrameData): string {
  if (f.protocol === "j1939") return "hl-j1939";
  if (f.protocol === "raw") return "hl-raw";
  if (f.protocol === "canopen") {
    const s = f.summary;
    // 心跳: Node X Operational/Pre-operational/... 或 明确 "Heartbeat"
    if (s.includes("Heartbeat") || s.includes("心跳")) return "hl-heartbeat";
    if (s.includes("SDO")) return "hl-sdo";
    if (s.includes("TPDO") || s.includes("RPDO") || s.includes("PDO")) return "hl-pdo";
    if (s.includes("NMT")) return "hl-nmt";
  }
  return "";
}

// ── 时间戳格式化 ──────────────────────────────────────────────

/** 将 u64 毫秒时间戳字符串格式化为 HH:MM:SS.mmm。 */
function formatTs(ts: string): string {
  const ms = Number(ts);
  if (Number.isNaN(ms)) return ts;
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const mmm = String(d.getMilliseconds()).padStart(3, "0");
  return `${hh}:${mm}:${ss}.${mmm}`;
}

// ── FrameTable 组件 ──────────────────────────────────────────

interface Props {
  /** 已过滤后的帧数组 (或未过滤的, 由调用方决定)。 */
  frames: FrameData[];
}

/**
 * CAN 帧流表格 — 虚拟滚动 + 协议高亮。
 *
 * - 固定行高 28px, 只渲染可视区 ± BUFFER_ROWS 行
 * - 自动滚动到底部 (新帧到达时, 除非用户向上滚动)
 * - 无帧时显示空态 "等待帧数据..."
 */
export function FrameTable({ frames }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 用户是否手动上滚 (暂停自动跟随)。 */
  const [userScrolledUp, setUserScrolledUp] = useState(false);
  /** 容器可视高度 (px), 用于计算可见行数。 */
  const [containerHeight, setContainerHeight] = useState(0);
  /** 当前 scrollTop, 用于计算起始行。 */
  const [scrollTop, setScrollTop] = useState(0);

  // ── ResizeObserver 获取容器高度 ──
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setContainerHeight(entry.contentRect.height);
      }
    });
    ro.observe(el);
    // 初始值
    setContainerHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  // ── 滚动事件: 更新 scrollTop + 判断是否在底部 ──
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    const distToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    setUserScrolledUp(distToBottom > SCROLL_THRESHOLD);
  }, []);

  // ── 新帧到达时自动滚到底部 ──
  const prevLenRef = useRef(0);
  useEffect(() => {
    if (frames.length > prevLenRef.current && !userScrolledUp) {
      const el = scrollRef.current;
      if (el) {
        // 用 requestAnimationFrame 确保 DOM 已更新高度后再滚
        requestAnimationFrame(() => {
          el.scrollTop = el.scrollHeight;
        });
      }
    }
    prevLenRef.current = frames.length;
  }, [frames.length, userScrolledUp]);

  // ── 虚拟滚动计算 ──
  const visibleCount = Math.ceil(containerHeight / ROW_HEIGHT);
  const startIndex = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - BUFFER_ROWS);
  const endIndex = Math.min(frames.length, startIndex + visibleCount + BUFFER_ROWS * 2);
  const visibleFrames = frames.slice(startIndex, endIndex);
  const topSpacer = startIndex * ROW_HEIGHT;
  const bottomSpacer = (frames.length - endIndex) * ROW_HEIGHT;

  // ── 空态 ──
  if (frames.length === 0) {
    return <div className="frame-table-empty">等待帧数据...</div>;
  }

  return (
    <div className="frame-table-container">
      <div
        ref={scrollRef}
        className="frame-table-scroll"
        onScroll={handleScroll}
      >
        <table className="frame-table">
          <thead>
            <tr>
              <th className="col-ts">时间</th>
              <th className="col-id">ID</th>
              <th className="col-dir">方向</th>
              <th className="col-data">数据</th>
              <th className="col-proto">协议</th>
              <th className="col-summary">摘要</th>
            </tr>
          </thead>
          <tbody>
            {/* 上方占位 */}
            {topSpacer > 0 && (
              <tr className="frame-table-spacer" aria-hidden>
                <td colSpan={6} style={{ height: topSpacer, padding: 0, border: "none" }} />
              </tr>
            )}
            {visibleFrames.map((f, i) => {
              const realIndex = startIndex + i;
              return (
                <tr
                  key={realIndex}
                  className={`${f.dir === "tx" ? "row-tx" : "row-rx"} ${highlightClass(f)}`}
                >
                  <td className="col-ts">{formatTs(f.ts)}</td>
                  <td className="col-id">{f.id}</td>
                  <td className="col-dir">{f.dir.toUpperCase()}</td>
                  <td className="col-data">{f.data}</td>
                  <td className="col-proto">{f.protocol}</td>
                  <td className="col-summary">{f.summary}</td>
                </tr>
              );
            })}
            {/* 下方占位 */}
            {bottomSpacer > 0 && (
              <tr className="frame-table-spacer" aria-hidden>
                <td colSpan={6} style={{ height: bottomSpacer, padding: 0, border: "none" }} />
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
