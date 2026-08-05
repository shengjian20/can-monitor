//! # CandumpLogger — candump -L 兼容的 CAN 帧日志记录器
//!
//! 以 can-utils `candump -L` 兼容的文本格式将原始 CAN 帧追加写入日志文件。
//!
//! ```text
//! (000.001234) vcan0 123#DEADBEEF
//! ```
//!
//! 格式约定 (与 candump -L 对齐):
//! - 时间戳: 相对 logger 创建时刻的 `秒.微秒`,外包圆括号 (candump -L 语义)
//! - 接口名: 帧来源接口名 (如 `vcan0` / `can0`)
//! - ID: 标准帧 3 位大写 hex,扩展帧 8 位大写 hex,后接 `#`
//! - 数据: 连续大写 hex,无空格 (与 candump 的 `ID#data` 一致)
//!
//! 日志开关与监控开关联动:关闭时不产生任何文件写入。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use can_types::{CanFrame, Result};

/// candump -L 兼容的 CAN 帧日志记录器。
///
/// # 字段
///
/// - `writer` : 缓冲文件写入器,`None` 表示日志已关闭 (不再写入)
/// - `enabled`: 日志开关,与监控开关联动,关闭时丢弃帧
/// - `start` / `start_systime`: 相对时间戳基准 (取自 logger 创建时刻)
///
/// # 输出示例
///
/// ```text
/// (000.000000) vcan0 123#DEADBEEF
/// (000.001234) vcan0 18FEF100#0102030405
/// ```
pub struct CandumpLogger {
    /// 缓冲文件写入器,`None` 表示日志已关闭。
    writer: Option<BufWriter<File>>,
    /// 日志开关,关闭时不写入。
    enabled: bool,
    /// 相对时间戳基准 (创建时刻),用于无帧时间戳时的回退。
    start: Instant,
    /// 相对时间戳基准的墙钟时间,用于有帧时间戳时计算相对秒数。
    start_systime: SystemTime,
}

impl CandumpLogger {
    /// 创建日志记录器,以追加模式打开指定文件。
    ///
    /// 文件不存在时自动创建;每次写入均追加到文件末尾。
    ///
    /// @param path 日志文件路径。
    /// @return 成功返回已打开的 [`CandumpLogger`];打开文件失败返回
    ///         [`can_types::CanError::Io`]。
    pub fn new(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(CandumpLogger {
            writer: Some(BufWriter::new(file)),
            enabled: true,
            start: Instant::now(),
            start_systime: SystemTime::now(),
        })
    }

    /// 将一帧格式化为 candump -L 兼容的一行 (不含换行)。
    ///
    /// 时间戳取 `elapsed` 参数,调用方负责计算相对基准的时长。
    ///
    /// @param frame   待格式化的 CAN 帧。
    /// @param iface   帧来源接口名。
    /// @param elapsed 相对 logger 创建时刻的时长。
    /// @return 形如 `(秒.微秒) 接口名 ID#数据` 的完整行字符串。
    fn format_line(frame: &CanFrame, iface: &str, elapsed: Duration) -> String {
        let id = frame.id();
        let id_str = if id.is_extended() {
            format!("{:08X}", id.raw_id())
        } else {
            format!("{:03X}", id.raw_id())
        };
        let data: String = frame.data().iter().map(|b| format!("{b:02X}")).collect();
        format!(
            "({:03}.{:06}) {iface} {id_str}#{data}",
            elapsed.as_secs(),
            elapsed.subsec_micros()
        )
    }

    /// 记录一帧到日志文件 (candump -L 兼容格式)。
    ///
    /// 时间戳优先取 [`CanFrame::timestamp`] 相对 logger 创建时刻的时长;
    /// 帧无时间戳时回退为当前时刻相对 logger 创建时刻的时长。
    ///
    /// @param frame 待记录的 CAN 帧。
    /// @param iface 帧来源接口名 (如 `vcan0`)。
    /// @return 写入成功返回 `Ok(())`;底层 IO 失败返回 [`can_types::CanError::Io`]。
    pub fn log_frame(&mut self, frame: &CanFrame, iface: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        let elapsed = match frame.timestamp() {
            Some(ts) => ts.duration_since(self.start_systime).unwrap_or(Duration::ZERO),
            None => self.start.elapsed(),
        };
        writeln!(writer, "{}", Self::format_line(frame, iface, elapsed))?;
        Ok(())
    }

    /// 设置日志开关,与监控开关联动。
    ///
    /// @param enabled `true` 开启写入,`false` 关闭写入 (丢弃帧)。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 查询日志开关当前状态。
    ///
    /// @return `true` 表示日志开启,`false` 表示关闭。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 冲刷缓冲内容到底层文件。
    ///
    /// @return 成功返回 `Ok(())`;冲刷失败返回 [`can_types::CanError::Io`]。
    pub fn flush(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }

    /// 冲刷并关闭日志文件。
    ///
    /// 关闭后 logger 不再写入任何内容,幂等 (重复调用无副作用)。
    ///
    /// @return 成功返回 `Ok(())`;冲刷失败返回 [`can_types::CanError::Io`]。
    pub fn close(&mut self) -> Result<()> {
        self.flush()?;
        self.writer = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::CanId;

    /// 构造带指定 ID 和数据的标准帧。
    fn std_frame(raw: u16, data: Vec<u8>) -> CanFrame {
        let id = CanId::new_standard(raw).unwrap();
        CanFrame::new(id, data).unwrap()
    }

    /// 构造带指定 ID 和数据的扩展帧。
    fn ext_frame(raw: u32, data: Vec<u8>) -> CanFrame {
        let id = CanId::new_extended(raw).unwrap();
        CanFrame::new(id, data).unwrap()
    }

    /// 生成临时日志文件路径 (进程号 + 纳秒,避免并发冲突)。
    fn temp_log_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("{name}-{}-{nanos}.log", std::process::id()))
    }

    /// 格式断言:标准帧 ID 3 位大写 hex,数据连续大写 hex,时间戳 `(秒.微秒)`。
    #[test]
    fn format_standard_frame_line() {
        let frame = std_frame(0x123, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let line = CandumpLogger::format_line(&frame, "vcan0", Duration::new(1, 234_567_000));
        assert_eq!(line, "(001.234567) vcan0 123#DEADBEEF");
    }

    /// 格式断言:扩展帧 ID 8 位大写 hex (含前导零填充)。
    #[test]
    fn format_extended_frame_line() {
        let frame = ext_frame(0x18FE_F100, vec![0x01, 0x02]);
        let line = CandumpLogger::format_line(&frame, "can1", Duration::new(0, 123_456_000));
        assert_eq!(line, "(000.123456) can1 18FEF100#0102");
    }

    /// 格式断言:小 ID 的扩展帧应 8 位补零,空数据帧尾部为 `#`。
    #[test]
    fn format_extended_padding_and_empty_data() {
        let frame = ext_frame(0x1F, vec![]);
        let line = CandumpLogger::format_line(&frame, "vcan1", Duration::ZERO);
        assert_eq!(line, "(000.000000) vcan1 0000001F#");
    }

    /// 格式断言:FD 帧统一按 `ID#数据` 输出 (不区分 FD 前缀)。
    #[test]
    fn format_fd_frame_line() {
        let id = CanId::new_extended(0x1F).unwrap();
        let frame = CanFrame::new_fd(id, (0..12).collect(), true, true).unwrap();
        let line = CandumpLogger::format_line(&frame, "can2", Duration::ZERO);
        assert_eq!(line, "(000.000000) can2 0000001F#000102030405060708090A0B");
    }

    /// 写入临时文件后读回,断言内容含完整 candump 格式行。
    #[test]
    fn write_frame_to_file_and_read_back() {
        let path = temp_log_path("write_frame");
        let mut logger = CandumpLogger::new(&path).unwrap();
        let frame = std_frame(0x123, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        logger.log_frame(&frame, "vcan0").unwrap();
        logger.close().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let line = content.trim();
        assert!(line.starts_with('('), "实际: {line}");
        assert!(line.contains(") vcan0 123#DEADBEEF"), "实际: {line}");
    }

    /// 追加模式:重新打开同一文件后继续写入,两行都应存在。
    #[test]
    fn reopen_appends_to_file() {
        let path = temp_log_path("append");
        {
            let mut logger = CandumpLogger::new(&path).unwrap();
            logger.log_frame(&std_frame(0x100, vec![0x01]), "vcan0").unwrap();
            logger.close().unwrap();
        }
        {
            let mut logger = CandumpLogger::new(&path).unwrap();
            logger.log_frame(&std_frame(0x200, vec![0x02]), "vcan0").unwrap();
            logger.close().unwrap();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(content.contains("100#01"), "实际: {content}");
        assert!(content.contains("200#02"), "实际: {content}");
    }

    /// 关闭开关后 log_frame 不产生任何写入。
    #[test]
    fn disabled_logger_writes_nothing() {
        let path = temp_log_path("disabled");
        let mut logger = CandumpLogger::new(&path).unwrap();
        logger.set_enabled(false);
        assert!(!logger.is_enabled());

        logger.log_frame(&std_frame(0x123, vec![0xAA]), "vcan0").unwrap();
        logger.close().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(content.is_empty());
    }

    /// 关闭开关再重新开启后,仅开启期间的帧被写入。
    #[test]
    fn reenabled_logger_writes_only_after_on() {
        let path = temp_log_path("reenabled");
        let mut logger = CandumpLogger::new(&path).unwrap();
        logger.set_enabled(false);
        logger.log_frame(&std_frame(0x100, vec![0x01]), "vcan0").unwrap();
        logger.set_enabled(true);
        logger.log_frame(&std_frame(0x200, vec![0x02]), "vcan0").unwrap();
        logger.close().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(content.contains("200#02"), "实际: {content}");
        assert!(!content.contains("100#01"), "关闭期间不应写入,实际: {content}");
    }

    /// 帧自带时间戳时,应相对 logger 创建时刻换算 (确定性的 2.345678 秒)。
    #[test]
    fn frame_timestamp_used_when_present() {
        let path = temp_log_path("timestamp");
        let mut logger = CandumpLogger::new(&path).unwrap();
        let mut frame = std_frame(0x300, vec![0x07]);
        frame.set_timestamp(logger.start_systime + Duration::new(2, 345_678_000));
        logger.log_frame(&frame, "vcan0").unwrap();
        logger.close().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(content.starts_with("(002.345678)"), "实际: {content}");
        assert!(content.contains("300#07"), "实际: {content}");
    }
}
