//! 存档磁盘服务（docs/architecture/06 §文件格式与落盘、§写入安全）：
//! 目录解析、原子写（同目录临时文件 + fsync + 系统级替换，不先删后
//! 改名）、写任务串行携带序号（旧序号不得覆盖新档）、手动档与自动档
//! 轮换（保留 3 份）、摘要列表与读删。设置 / 草稿文件共用同一原子写
//! 工具。故障注入旋钮 `ZTW_SAVE_FAULT`（docs/architecture/08 原型 C
//! 存档条款；清单为单一事实源，测试以互锁探针对账——模式仿
//! host_ipc::FAULT_KNOBS）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use ztw_api::save::{SaveData, decode};

/// 自动档轮换保留数量（docs/architecture/06 §文件格式与落盘）。
pub const AUTO_SLOTS: u32 = 3;

/// 存档写入故障注入旋钮清单（环境变量 `ZTW_SAVE_FAULT` 传单个旋钮名）。
/// 旋钮语义（探针测试逐项对照，见 tests/c1_save_load.rs）：
/// - `fail_tmp_write`：临时文件写入失败（磁盘满 / 权限模拟）。
/// - `corrupt_payload`：落盘前篡改载荷一字节（校验和门禁的检测对象）。
/// - `crash_before_replace`：原子替换前进程终止（旧档必须原样保留）。
/// - `crash_after_replace`：原子替换后进程终止（新档必须完整可用）。
/// - `stale_seq`：执行写前模拟更晚到达的写请求（旧序号被取代）。
pub const SAVE_FAULT_KNOBS: &[&str] = &[
    "fail_tmp_write",
    "corrupt_payload",
    "crash_before_replace",
    "crash_after_replace",
    "stale_seq",
];

fn knob_from_env() -> Option<String> {
    std::env::var("ZTW_SAVE_FAULT").ok()
}

/// Unix 毫秒时间戳（存档创建时间；发给前端时转字符串）。
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 面向前端的存档摘要（list_saves）。64 位值转字符串：经 Tauri IPC 会
/// 过 JS Number，毫秒时间戳超 2^53 精度边界。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SaveSummary {
    /// 相对存档根的稳定 id（正斜杠分隔，如 `m3-trade/manual/...json`）。
    pub id: String,
    pub scenario: String,
    pub name: Option<String>,
    pub auto: bool,
    pub tick: u64,
    pub created_at_ms: String,
    pub language: String,
    /// 摘要是否通过全部门禁（版本 / 校验和 / 结构）；false 时仍列出
    /// （灰条），`error` 给一句话原因。
    pub ok: bool,
    pub error: Option<String>,
}

/// 已分配的写任务：目标文件 + 该目标的最新序号（docs 06：写入任务串行，
/// 携带存档序号；旧请求不得覆盖新版本）。
#[derive(Debug, Clone)]
pub struct SaveTicket {
    target: PathBuf,
    seq: u64,
    auto: bool,
}

struct WriteGate {
    /// 目标文件 → 最新已发序号。
    issued: HashMap<PathBuf, u64>,
}

/// 存档存储：`<app_data>/saves` 之下的手动 / 自动档树。故障旋钮在构造
/// 时确定（生产路径读一次 `ZTW_SAVE_FAULT` 环境变量；测试经 `with_fault`
/// 显式注入——同一测试二进制内多测试并行，不得在进程内改环境变量）。
pub struct SaveStore {
    root: PathBuf,
    gate: Mutex<WriteGate>,
    fault: Option<String>,
}

impl SaveStore {
    pub fn new(root: PathBuf) -> SaveStore {
        SaveStore::with_fault(root, knob_from_env().as_deref())
    }

    pub fn with_fault(root: PathBuf, fault: Option<&str>) -> SaveStore {
        SaveStore {
            root,
            gate: Mutex::new(WriteGate {
                issued: HashMap::new(),
            }),
            fault: fault.map(String::from),
        }
    }

    fn knob(&self, name: &str) -> bool {
        self.fault.as_deref() == Some(name)
    }

    fn alloc_ticket(&self, target: PathBuf, auto: bool) -> SaveTicket {
        let mut g = self.gate.lock().expect("存档写入门锁");
        let seq = g.issued.get(&target).copied().unwrap_or(0) + 1;
        g.issued.insert(target.clone(), seq);
        SaveTicket { target, seq, auto }
    }

    /// 手动档槽位：`saves/<场景>/manual/<创建毫秒>-<序号>.json`（序号
    /// 后缀防同毫秒碰撞）。
    pub fn ticket_manual(&self, scenario: &str, created_ms: u64) -> SaveTicket {
        let target = self
            .root
            .join(scenario)
            .join("manual")
            .join(format!("{created_ms}-pending.json"));
        self.alloc_ticket(target, false)
    }

    /// 自动档轮换槽位：`saves/<场景>/auto/auto-<n>.json`，n ∈ 1..=3；
    /// 有空槽用空槽，全满覆 created_at 最旧者。文件名最终落定在 write。
    pub fn ticket_auto(&self, scenario: &str) -> SaveTicket {
        let dir = self.root.join(scenario).join("auto");
        let mut free: Option<u32> = None;
        let mut oldest: Option<(u64, u32)> = None; // (created_at, slot)
        for n in 1..=AUTO_SLOTS {
            let p = dir.join(format!("auto-{n}.json"));
            if !p.exists() {
                free.get_or_insert(n);
                continue;
            }
            let created = fs::read_to_string(&p)
                .ok()
                .and_then(|t| raw_header(&t))
                .and_then(|h| h.get("created_at_ms").and_then(Value::as_u64))
                .unwrap_or(u64::MAX);
            if oldest.is_none_or(|(c, _)| created < c) {
                oldest = Some((created, n));
            }
        }
        let n = free.or_else(|| oldest.map(|(_, n)| n)).unwrap_or(1);
        self.alloc_ticket(dir.join(format!("auto-{n}.json")), true)
    }

    /// 执行写（调用方经 spawn_blocking）。新鲜度门：序号不再是该目标
    /// 最新值时拒绝执行替换（旧请求不覆盖新档）。成功返回存档相对 id。
    pub fn write(
        &self,
        ticket: &SaveTicket,
        text: &str,
        created_ms: u64,
    ) -> Result<String, String> {
        if self.knob("stale_seq") {
            let mut g = self.gate.lock().expect("存档写入门锁");
            *g.issued.get_mut(&ticket.target).expect("票已分配") += 1;
        }
        {
            let g = self.gate.lock().expect("存档写入门锁");
            if g.issued.get(&ticket.target) != Some(&ticket.seq) {
                return Err("写入请求已被更新的存档请求取代（旧序号不覆盖新档）".to_string());
            }
        }
        let mut bytes = text.to_string();
        if self.knob("corrupt_payload") {
            // 篡改 save 段内 tick 的一位数字：JSON 仍合法、载荷已变——
            // 校验和门禁（而非 JSON 解析）的检测对象。
            if let Some(pos) = bytes.find("\"tick\":") {
                let at = pos + "\"tick\":".len();
                if let Some(b) = bytes[at..].bytes().next()
                    && b.is_ascii_digit()
                {
                    // 安全替换（ASCII 数字位）。
                    let flipped = if b == b'0' { b'1' } else { b'0' };
                    unsafe { bytes.as_bytes_mut()[at] = flipped };
                }
            }
        }
        if self.knob("fail_tmp_write") {
            return Err("注入故障：临时文件写入失败（磁盘满模拟）".to_string());
        }
        let dir = ticket.target.parent().expect("目标必有父目录");
        fs::create_dir_all(dir).map_err(|e| format!("创建存档目录失败：{e}"))?;
        // 最终文件名带创建毫秒（手动档），先写同目录临时文件再原子替换。
        let final_path = if ticket.auto {
            ticket.target.clone()
        } else {
            dir.join(format!("{created_ms}-{}.json", ticket.seq))
        };
        let tmp = final_path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败：{e}"))?;
            f.write_all(bytes.as_bytes())
                .map_err(|e| format!("写入临时文件失败：{e}"))?;
            f.sync_all().map_err(|e| format!("刷新临时文件失败：{e}"))?;
        }
        if self.knob("crash_before_replace") {
            eprintln!("[ZTW_SAVE_FAULT] crash_before_replace：替换前终止");
            std::process::abort();
        }
        fs::rename(&tmp, &final_path).map_err(|e| format!("原子替换存档失败：{e}"))?;
        sync_dir(dir);
        if self.knob("crash_after_replace") {
            eprintln!("[ZTW_SAVE_FAULT] crash_after_replace：替换后终止");
            std::process::abort();
        }
        // 相对 id（正斜杠分隔，跨平台稳定）。
        let rel = final_path
            .strip_prefix(&self.root)
            .expect("最终路径在存档根之下");
        let id = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        Ok(id)
    }

    /// 摘要列表（跨场景；损坏档 ok=false 附原因，不剔除）。
    pub fn list(&self) -> Vec<SaveSummary> {
        let mut out = Vec::new();
        let Ok(scenarios) = fs::read_dir(&self.root) else {
            return out;
        };
        let mut scenario_names: Vec<String> = scenarios
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        scenario_names.sort();
        for s in scenario_names {
            for kind in ["manual", "auto"] {
                let dir = self.root.join(&s).join(kind);
                let Ok(files) = fs::read_dir(&dir) else {
                    continue;
                };
                let mut names: Vec<String> = files
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                names.sort();
                for name in names {
                    let id = format!("{s}/{kind}/{name}");
                    out.push(self.summarize(&id));
                }
            }
        }
        out
    }

    /// 单档摘要（含全部门禁复核；损坏档 ok=false 附原因）。
    pub fn summarize(&self, id: &str) -> SaveSummary {
        let rel = id.split('/').collect::<Vec<_>>();
        let scenario = rel.first().map(|s| s.to_string()).unwrap_or_default();
        let auto = rel.get(1) == Some(&"auto");
        let fallback = SaveSummary {
            id: id.to_string(),
            scenario,
            name: None,
            auto,
            tick: 0,
            created_at_ms: "0".to_string(),
            language: "js".to_string(),
            ok: false,
            error: Some("文件不可读".to_string()),
        };
        let Ok(text) = self.read_text(id) else {
            return fallback;
        };
        match decode(&text) {
            Ok(d) => SaveSummary {
                id: id.to_string(),
                scenario: d.scenario_id,
                name: d.name,
                auto,
                tick: d.tick,
                created_at_ms: d.created_at_ms.to_string(),
                language: d.program.language,
                ok: true,
                error: None,
            },
            Err(e) => {
                let mut s = fallback;
                // 门禁失败但 JSON 头可读时尽量带出字段（灰条展示）。
                if let Some(h) = raw_header(&text) {
                    s.name = h.get("name").and_then(Value::as_str).map(String::from);
                    s.tick = h.get("tick").and_then(Value::as_u64).unwrap_or(0);
                    s.created_at_ms = h
                        .get("created_at_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .to_string();
                }
                s.error = Some(e);
                s
            }
        }
    }

    fn resolve(&self, id: &str) -> Result<PathBuf, String> {
        let p = Path::new(id);
        if p.is_absolute() || id.contains("..") || id.contains('\\') {
            return Err(format!("非法存档 id「{id}」"));
        }
        Ok(self.root.join(p))
    }

    fn read_text(&self, id: &str) -> Result<String, String> {
        let path = self.resolve(id)?;
        fs::read_to_string(&path).map_err(|e| format!("读取存档失败：{e}"))
    }

    /// 读取并解码（load_game 用；走 api 层全部门禁）。
    pub fn read(&self, id: &str) -> Result<SaveData, String> {
        let text = self.read_text(id)?;
        decode(&text)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.resolve(id)?;
        fs::remove_file(&path).map_err(|e| format!("删除存档失败：{e}"))
    }
}

/// 解析文件顶层 JSON（摘要兜底；不做门禁）。
fn raw_header(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok()
}

/// 目录刷新（docs 06：平台适用的目录刷新分别验证）。Unix 打开目录
/// sync；Windows 目录句柄不支持 fsync，跳过（rename 本身原子）。
fn sync_dir(p: &Path) {
    #[cfg(unix)]
    {
        if let Ok(f) = fs::File::open(p) {
            let _ = f.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = p;
    }
}

// ---------------------------------------------------------------------------
// 设置 / 草稿文件（同一原子写纪律；docs 06 §文件格式与落盘）
// ---------------------------------------------------------------------------

/// 原子写 JSON 文件（临时 + fsync + 替换 + 目录刷新）。
pub fn write_json_atomic(path: &Path, v: &Value) -> Result<(), String> {
    let dir = path.parent().expect("路径必有父目录");
    fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败：{e}"))?;
        f.write_all(serde_json::to_string(v).unwrap_or_default().as_bytes())
            .map_err(|e| format!("写入临时文件失败：{e}"))?;
        f.sync_all().map_err(|e| format!("刷新临时文件失败：{e}"))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("原子替换失败：{e}"))?;
    sync_dir(dir);
    Ok(())
}

/// 读 JSON 文件（缺失返回 None；损坏按坏文件报错，不静默吞）。
pub fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("设置文件损坏：{e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旋钮互锁：清单即文档，任何增删须同步探针测试（模式仿
    /// a1_fault_parity 的 every_knob_has_a_parity_probe）。
    #[test]
    fn every_knob_documented() {
        for k in SAVE_FAULT_KNOBS {
            assert!(
                matches!(
                    *k,
                    "fail_tmp_write"
                        | "corrupt_payload"
                        | "crash_before_replace"
                        | "crash_after_replace"
                        | "stale_seq"
                ),
                "旋钮 {k} 缺少语义注释与探针对照"
            );
        }
    }
}
