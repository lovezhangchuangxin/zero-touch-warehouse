//! 存档信封（docs/architecture/06 §文件格式与落盘）：JSON 信封 + FNV-1a 64
//! 校验和 + 版本门禁（存档格式 / 模拟规则 / 协议三道硬门，运行时版本仅
//! 记录不拦截）。不兼容存档明确报错、不静默迁移（1.0 前策略）。存档文件
//! 只经 Rust 读写——PRNG 状态字等 u64 以 JSON 数字无损存取；任何要发给
//! 前端的 64 位值由桌面层转字符串（JS Number 精度边界）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ztw_model::MemValue;
use ztw_sim::{RULES_VERSION, World, WorldSnapshot};

use crate::harness::PlayerProgram;
use crate::protocol::PROTOCOL_VERSION;

/// 存档文件格式版本（信封布局变更时递增；旧档读档明确报错不迁移）。
pub const SAVE_FORMAT_VERSION: u32 = 1;
/// 信封 magic（快速识别非本游戏文件）。
pub const SAVE_MAGIC: &str = "ztw-save";
/// 运行时版本（记录用，不拦截——语义门禁是格式 / 规则 / 协议三道）。
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 信封：版本与校验和 + 存档本体（`save` 段）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveFile {
    pub magic: String,
    pub format_version: u32,
    pub rules_version: String,
    pub protocol_version: u32,
    pub runtime_version: String,
    /// `save` 段规范化 JSON 字节的 FNV-1a 64（十进制字符串——只作文件
    /// 内部校验，避免任何 u64 数字歧义）。
    pub checksum: String,
    pub save: SaveData,
}

/// 存档本体（docs/architecture/06 存档内容表）。快照数据面：世界镜像
/// DTO + memory 线值与修订号 + 已加载程序；未加载草稿单独一段、不冒充
/// 正在运行的代码。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveData {
    pub scenario_id: String,
    /// 玩家可见显示名（自动档为 None，显示回退到场景名与时间）。
    pub name: Option<String>,
    /// 存档创建时间（Unix 毫秒）。
    pub created_at_ms: u64,
    /// 世界指纹（安全点 state_hash；读档恢复后对账）。
    pub state_hash: u64,
    /// 存档时刻 tick（列表展示与自动档排序用）。
    pub tick: u64,
    pub world: WorldSnapshot,
    pub memory: SaveMemory,
    pub program: SaveProgram,
    /// 编辑器草稿段（结构由前端定义、原样透传）。
    #[serde(default)]
    pub drafts: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveMemory {
    pub root: MemValue,
    pub revision: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveProgram {
    pub files: BTreeMap<String, String>,
    pub entry: String,
    /// "js" / "py"（桌面层负责与宿主二进制对齐）。
    pub language: String,
}

impl SaveData {
    /// 安全点组装（纯函数，无宿主可测）：世界与 memory 来自同一安全点
    /// （docs 06），指纹在序列化前落定。
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        world: &World,
        memory_root: MemValue,
        memory_revision: u64,
        program: &PlayerProgram,
        language: &str,
        scenario_id: &str,
        name: Option<String>,
        drafts: Option<Value>,
        created_at_ms: u64,
    ) -> SaveData {
        SaveData {
            scenario_id: scenario_id.to_string(),
            name,
            created_at_ms,
            state_hash: world.state_hash(),
            tick: world.tick,
            world: world.to_snapshot(),
            memory: SaveMemory {
                root: memory_root,
                revision: memory_revision,
            },
            program: SaveProgram {
                files: program.files.clone(),
                entry: program.entry.clone(),
                language: language.to_string(),
            },
            drafts,
        }
    }

    /// 世界恢复 + 指纹对账（镜像漏字段的双保险：sim 往返测试之外，读档
    /// 运行时再核一次 state_hash）。
    pub fn restore_world(&self) -> Result<World, String> {
        let w = World::from_snapshot(self.world.clone())?;
        let actual = w.state_hash();
        if actual != self.state_hash {
            return Err(format!(
                "世界指纹对账失败：存档 {}，恢复后 {actual}（存档损坏或格式不一致）",
                self.state_hash
            ));
        }
        Ok(w)
    }
}

/// FNV-1a 64（与 sim state_hash 同款混合，私有工具——文件内部校验）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 编码为信封 JSON 文本。校验和取 `save` 段经 serde_json::Value 规范化
/// （键按序、数字原样）后的字节——与 decode 的复算路径完全同源。
pub fn encode(data: &SaveData) -> String {
    let save = serde_json::to_value(data).expect("SaveData 序列化不失败");
    let checksum = fnv1a64(save.to_string().as_bytes()).to_string();
    let file = SaveFile {
        magic: SAVE_MAGIC.to_string(),
        format_version: SAVE_FORMAT_VERSION,
        rules_version: RULES_VERSION.to_string(),
        protocol_version: PROTOCOL_VERSION,
        runtime_version: RUNTIME_VERSION.to_string(),
        checksum,
        save: data.clone(),
    };
    serde_json::to_string(&file).expect("存档信封序列化不失败")
}

/// 解码并执行全部门禁：magic → 格式 / 规则 / 协议版本 → 校验和 → 结构。
/// 任何一步失败返回可读中文错误（旧档不迁移，docs 06）。
pub fn decode(text: &str) -> Result<SaveData, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("存档不是合法 JSON：{e}"))?;
    if v.get("magic").and_then(Value::as_str) != Some(SAVE_MAGIC) {
        return Err("不是本游戏的存档文件（magic 不符）".to_string());
    }
    let gate = |key: &str, actual: String, expect: String| -> Option<String> {
        (actual != expect).then(|| {
            format!(
                "存档{key}不匹配：文件 {actual}，当前 {expect}——本版本不迁移旧存档，请用当前版本重新开局"
            )
        })
    };
    let file_format = v
        .get("format_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "存档缺少格式版本".to_string())?;
    if let Some(e) = gate(
        "格式版本",
        format!("v{file_format}"),
        format!("v{SAVE_FORMAT_VERSION}"),
    ) {
        return Err(e);
    }
    if let Some(e) = gate(
        "规则版本",
        v.get("rules_version")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        RULES_VERSION.to_string(),
    ) {
        return Err(e);
    }
    let file_protocol = v
        .get("protocol_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "存档缺少协议版本".to_string())?;
    if let Some(e) = gate(
        "协议版本",
        format!("v{file_protocol}"),
        format!("v{PROTOCOL_VERSION}"),
    ) {
        return Err(e);
    }
    let Some(save_v) = v.get("save") else {
        return Err("存档缺少 save 段".to_string());
    };
    let checksum = fnv1a64(save_v.to_string().as_bytes()).to_string();
    let expect = v
        .get("checksum")
        .and_then(Value::as_str)
        .ok_or_else(|| "存档缺少校验和".to_string())?;
    if checksum != expect {
        return Err("存档校验和不符（文件损坏），已拒绝读取".to_string());
    }
    serde_json::from_value::<SaveData>(save_v.clone()).map_err(|e| format!("存档内容结构不符：{e}"))
}
