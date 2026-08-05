import { useState, useCallback } from "react";
import type { Api } from "../types";

interface Props {
  api: Api;
  disabled: boolean;
}

interface SendFrame {
  id: string;
  ext: boolean;
  data: string;
}

/**
 * CAN 帧发送面板 —— ID (hex/dec), 扩展帧勾选, data (空格分隔 hex)。
 *
 * 发送后清空 data 保留 ID; 非法输入显示中文错误。
 */
export function SendPanel({ api, disabled }: Props) {
  const [idStr, setIdStr] = useState("");
  const [ext, setExt] = useState(false);
  const [dataStr, setDataStr] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  // ── 校验 ──────────────────────────────────────────────────────

  /** 解析 ID 字符串为数值 (支持 0x 前缀 hex 和十进制)。 */
  const parseId = useCallback((raw: string): number | null => {
    const s = raw.trim();
    if (!s) return null;

    let n: number;
    if (s.startsWith("0x") || s.startsWith("0X")) {
      n = parseInt(s, 16);
    } else {
      n = parseInt(s, 10);
    }

    if (isNaN(n)) return null;

    // 标准帧 11 位 (0x000-0x7FF), 扩展帧 29 位 (0x00000000-0x1FFFFFFF)。
    if (n < 0) return null;
    return n;
  }, []);

  /** 校验 data 字符串: 空格分隔的 hex 字节, 每字节 00-FF。 */
  const parseData = useCallback(
    (raw: string): Uint8Array | null => {
      const s = raw.trim();
      if (!s) return new Uint8Array(0);

      const parts = s.split(/\s+/);
      const bytes: number[] = [];

      for (const part of parts) {
        if (!part) continue;
        const n = parseInt(part, 16);
        if (isNaN(n) || n < 0 || n > 255) return null;
        bytes.push(n);
      }

      // CAN 帧最大 8 字节 (经典) / 64 字节 (FD, 暂不支持)。
      if (bytes.length > 8) return null;
      return new Uint8Array(bytes);
    },
    []
  );

  // ── 发送 ──────────────────────────────────────────────────────

  const handleSend = useCallback(async () => {
    setError(null);

    // 校验 ID。
    const idNum = parseId(idStr);
    if (idNum === null) {
      setError("ID 格式错误: 请输入十六进制 (0x123) 或十进制数字");
      return;
    }

    // 校验 ID 范围。
    const maxId = ext ? 0x1fffffff : 0x7ff;
    if (idNum > maxId) {
      setError(
        ext
          ? `扩展帧 ID 超出范围 (最大 0x${maxId.toString(16).toUpperCase()})`
          : `标准帧 ID 超出范围 (最大 0x${maxId.toString(16).toUpperCase()})`
      );
      return;
    }

    // 校验 data。
    const dataBytes = parseData(dataStr);
    if (dataBytes === null) {
      setError("数据格式错误: 请输入空格分隔的十六进制字节 (如 01 02 FF), 最多 8 字节");
      return;
    }

    // 格式化: ID → "0x" 前缀 hex, data → 空格分隔大写 hex。
    const frame: SendFrame = {
      id: `0x${idNum.toString(16).toUpperCase()}`,
      ext,
      data: Array.from(dataBytes)
        .map((b) => b.toString(16).padStart(2, "0").toUpperCase())
        .join(" "),
    };

    setSending(true);
    try {
      await api.sendFrame(frame);
      // 发送成功: 清空 data, 保留 ID。
      setDataStr("");
      setError(null);
    } catch (e) {
      setError(`发送失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSending(false);
    }
  }, [idStr, ext, dataStr, api, parseId, parseData]);

  // Enter 发送。
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !sending && !disabled) {
      handleSend();
    }
  };

  return (
    <div className="send-panel" onKeyDown={handleKeyDown}>
      <div className="send-row">
        {/* ID 输入 */}
        <label className="send-label">
          ID
          <input
            type="text"
            className="send-input send-input-id"
            placeholder="0x181"
            value={idStr}
            onChange={(e) => setIdStr(e.target.value)}
            disabled={disabled || sending}
          />
        </label>

        {/* 扩展帧勾选 */}
        <label className="send-checkbox-label">
          <input
            type="checkbox"
            checked={ext}
            onChange={(e) => setExt(e.target.checked)}
            disabled={disabled || sending}
          />
          <span>EXT</span>
        </label>

        {/* Data 输入 */}
        <label className="send-label send-label-data">
          Data
          <input
            type="text"
            className="send-input send-input-data"
            placeholder="01 02 03 04"
            value={dataStr}
            onChange={(e) => setDataStr(e.target.value)}
            disabled={disabled || sending}
          />
        </label>

        {/* 发送按钮 */}
        <button
          className="btn btn-send"
          onClick={handleSend}
          disabled={disabled || sending || !idStr.trim()}
        >
          {sending ? "发送中..." : "发送"}
        </button>
      </div>

      {/* 错误提示 */}
      {error && <div className="send-error">{error}</div>}
    </div>
  );
}
