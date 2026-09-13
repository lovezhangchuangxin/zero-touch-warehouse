//! A0 IPC 协议：有长度前缀的 JSON 请求-回复（docs/architecture/03）。
//!
//! 传输走宿主进程 stdin/stdout。A0 不实现 host_epoch / 请求去重 /
//! 重复消息机制（A1 范围），`request_id` 仅用于完成消息引用与协议健全性检查。
//!
//! 变更型调用（动作受理、管理操作）与 memory 读写走本协议；数据查询走
//! 宿主本地镜像，仅在镜像失效时经 `mirror.fetch` 回退重建。

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;

/// 帧硬上限之上的“物理”上限：防御长度前缀被破坏后的巨量读取。
const ABSOLUTE_FRAME_CAP: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 主进程 → 宿主
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MainFrame {
    /// 发起一次执行。init 携带完整源码并重建执行环境；loop 复用环境。
    Exec {
        /// 协议版本（消息头要求，docs/architecture/03 IPC 提交协议）。
        #[serde(default = "protocol_version")]
        v: u32,
        execution_id: u64,
        /// "init" | "loop"
        kind: String,
        tick: u64,
        /// 运行时内中断 deadline（毫秒），由主进程下发。
        budget_ms: u64,
        /// 仅 init：完整玩家源码（只接受源码文本，不接受预编译产物）。
        source: Option<String>,
        /// 全量查询镜像（JSON 字符串）。init 与每 tick 均携带。
        mirror: Option<String>,
        /// memory 会话代次：宿主据此失效旧句柄缓存。
        memory_gen: u64,
    },
    /// 对 Game 请求的回复。`result` 为统一结果 JSON（见下）。
    Reply {
        #[serde(default = "protocol_version")]
        v: u32,
        request_id: u64,
        result: String,
    },
}

pub fn protocol_version() -> u32 {
    PROTOCOL_VERSION
}

// ---------------------------------------------------------------------------
// 宿主 → 主进程
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostFrame {
    /// 玩家代码发起的变更型调用 / memory 读写 / 日志 / 镜像重建。
    Request {
        #[serde(default = "protocol_version")]
        v: u32,
        request_id: u64,
        op: String,
        payload: String,
    },
    /// 执行完成；引用最后请求号。宿主按 FIFO 串行发请求，此前的请求
    /// 必已处理完（docs/architecture/03 IPC 提交协议）。
    Complete {
        #[serde(default = "protocol_version")]
        v: u32,
        last_request_id: u64,
        /// 仅 init 有意义：入口 loop 是否为函数。
        has_loop: bool,
        stats: ExecStats,
    },
    /// 执行故障。
    /// class: "script"（脚本级：语法错误 / 未捕获异常 / 中断）|
    ///        "environment"（环境级：JS 内存超限等，运行时状态不可信）|
    ///        "protocol"（宿主侧协议错误，进程将退出）
    Fault {
        #[serde(default = "protocol_version")]
        v: u32,
        class: String,
        code: String,
        message: String,
        stack: String,
        last_request_id: u64,
        stats: ExecStats,
    },
}

/// 宿主侧单次执行统计（量测用；不参与语义）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecStats {
    pub ipc_count: u64,
    pub ipc_total_us: u64,
    pub ipc_max_us: u64,
    pub mirror_parse_us: u64,
    pub mirror_apply_us: u64,
    pub mirror_rebuilds: u64,
}

// ---------------------------------------------------------------------------
// Game 请求 op 与统一结果 JSON
// ---------------------------------------------------------------------------

/// 统一回复：`{"ok":true, ...}` 或 `{"ok":false,"code":..,"message":..}`。
/// 绑定层把 !ok 转为带 code 的异常。
pub fn ok_result(fields: serde_json::Value) -> String {
    let mut obj = serde_json::Map::new();
    // 先并 fields，再写入 ok：调用方即使传入 "ok":false 也不能翻转语义。
    if let serde_json::Value::Object(m) = fields {
        for (k, v) in m {
            obj.insert(k, v);
        }
    }
    obj.insert("ok".into(), serde_json::Value::Bool(true));
    serde_json::Value::Object(obj).to_string()
}

pub fn err_result(code: &str, message: &str) -> String {
    serde_json::json!({
        "ok": false,
        "code": code,
        "message": message,
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// 帧编解码：4 字节大端长度前缀 + JSON
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    /// 声明长度超过上限：主进程不得解码（docs/architecture/03 主进程边界）。
    Oversized {
        declared: u64,
        limit: u64,
    },
    /// 帧边界处的干净 EOF（对端关闭）。进程退出（含被杀）的常态信号。
    Closed,
    /// 帧中途 EOF 或长度前缀不完整：协议破坏。
    Truncated,
    Malformed(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "io: {e}"),
            FrameError::Oversized { declared, limit } => {
                write!(f, "frame {declared}B exceeds limit {limit}B")
            }
            FrameError::Closed => write!(f, "connection closed"),
            FrameError::Truncated => write!(f, "frame truncated"),
            FrameError::Malformed(m) => write!(f, "malformed frame: {m}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(e: std::io::Error) -> Self {
        FrameError::Io(e)
    }
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, frame: &T) -> Result<(), FrameError> {
    let body = serde_json::to_vec(frame).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if body.len() as u64 > ABSOLUTE_FRAME_CAP {
        return Err(FrameError::Oversized {
            declared: body.len() as u64,
            limit: ABSOLUTE_FRAME_CAP,
        });
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// 读取一帧。`limit` 是调用方配置的消息上限（主进程与宿主各自执行）。
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(
    r: &mut R,
    limit: u64,
) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    read_exact_or_eof(r, &mut len_buf)?;
    let declared = u32::from_be_bytes(len_buf) as u64;
    let effective_limit = limit.min(ABSOLUTE_FRAME_CAP);
    if declared > effective_limit {
        return Err(FrameError::Oversized {
            declared,
            limit: effective_limit,
        });
    }
    let mut body = vec![0u8; declared as usize];
    r.read_exact(&mut body).map_err(|_| FrameError::Truncated)?;
    serde_json::from_slice(&body).map_err(|e| FrameError::Malformed(e.to_string()))
}

fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<(), FrameError> {
    let mut off = 0;
    while off < buf.len() {
        match r.read(&mut buf[off..])? {
            0 => {
                return Err(if off == 0 {
                    FrameError::Closed // 帧边界干净关闭
                } else {
                    FrameError::Truncated // 帧中途断开
                });
            }
            n => off += n,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = MainFrame::Exec {
            v: PROTOCOL_VERSION,
            execution_id: 1,
            kind: "loop".into(),
            tick: 7,
            budget_ms: 200,
            source: None,
            mirror: Some("{}".into()),
            memory_gen: 3,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &f).unwrap();
        assert!(buf.len() > 4); // 长度前缀 + JSON 体
        let back: MainFrame = read_frame(&mut buf.as_slice(), 1024 * 1024).unwrap();
        match back {
            MainFrame::Exec { tick, .. } => assert_eq!(tick, 7),
            _ => panic!(),
        }
    }

    #[test]
    fn oversize_rejected_before_decode() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        let err = read_frame::<_, MainFrame>(&mut buf.as_slice(), 1024).unwrap_err();
        assert!(matches!(err, FrameError::Oversized { .. }));
    }

    #[test]
    fn closed_vs_truncated_distinguished() {
        // 帧边界干净关闭（0 字节前缀读取）→ Closed。
        let empty: &[u8] = &[];
        assert!(matches!(
            read_frame::<_, HostFrame>(&mut &empty[..], 1024).unwrap_err(),
            FrameError::Closed
        ));
        // 长度前缀中途断开 → Truncated。
        let partial = [0u8, 0, 0, 5, 0xAB];
        assert!(matches!(
            read_frame::<_, HostFrame>(&mut &partial[..], 1024).unwrap_err(),
            FrameError::Truncated
        ));
        // 体读取中途断开 → Truncated。
        let mut body_only = Vec::new();
        body_only.extend_from_slice(&4u32.to_be_bytes());
        body_only.extend_from_slice(b"{\"t");
        assert!(matches!(
            read_frame::<_, HostFrame>(&mut &body_only[..], 1024).unwrap_err(),
            FrameError::Truncated
        ));
    }

    #[test]
    fn version_field_defaults_and_roundtrip() {
        // 无 v 字段的旧帧可解析（default），新帧携带当前版本。
        let legacy = br#"{"type":"Reply","request_id":1,"result":"{}"}"#;
        let mut buf = Vec::new();
        buf.extend_from_slice(&(legacy.len() as u32).to_be_bytes());
        buf.extend_from_slice(legacy);
        let f: MainFrame = read_frame(&mut buf.as_slice(), 1024).unwrap();
        match f {
            MainFrame::Reply { v, .. } => assert_eq!(v, PROTOCOL_VERSION),
            _ => panic!(),
        }
    }

    #[test]
    fn ok_result_cannot_be_flipped() {
        let r = ok_result(serde_json::json!({ "ok": false }));
        assert!(r.contains("\"ok\":true"));
    }

    #[test]
    fn malformed_rejected() {
        let mut buf = Vec::new();
        let body = b"not json";
        buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
        buf.extend_from_slice(body);
        let err = read_frame::<_, HostFrame>(&mut buf.as_slice(), 1024).unwrap_err();
        assert!(matches!(err, FrameError::Malformed(_)));
    }
}
