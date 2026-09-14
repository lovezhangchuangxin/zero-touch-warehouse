//! 纯数据类型：实体、坐标、订单、结果码与 memory 线值。
//! 对应 docs/architecture/01-overview.md 的 `crates/model`。

use serde::{Deserialize, Serialize};

/// 全局自增对象 id：跨类型唯一、单调递增、永不复用（docs/game-design/08）。
pub type Id = u64;

/// 权威经济值：金币千分（i64 定点）。f64 不得进入世界状态
/// （docs/architecture/02 确定性规则·数学）。
pub type MilliGold = i64;

pub const GOLD_MILLI_SCALE: MilliGold = 1000;

/// 坐标。原点左上，x 向右，y 向下（值对象，无 id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    pub fn new(x: i32, y: i32) -> Self {
        Position { x, y }
    }
    pub fn adjacent(&self, other: &Position) -> bool {
        let (dx, dy) = (self.x - other.x, self.y - other.y);
        dx.abs() + dy.abs() == 1
    }
    pub fn step(&self, dx: i32, dy: i32) -> Position {
        Position::new(self.x + dx, self.y + dy)
    }
}

/// 方向常量（与 API 的 Game.NORTH/SOUTH/WEST/EAST 对应）。
pub const NORTH: (i32, i32) = (0, -1);
pub const SOUTH: (i32, i32) = (0, 1);
pub const WEST: (i32, i32) = (-1, 0);
pub const EAST: (i32, i32) = (1, 0);

// ---------------------------------------------------------------------------
// 结果码（docs/game-design/08-api-design.md 示例清单；最终清单随实现冻结）
// ---------------------------------------------------------------------------

pub mod codes {
    pub const OK: &str = "OK";
    pub const ARRIVED: &str = "ARRIVED";
    pub const ALREADY_ACTED: &str = "ALREADY_ACTED";
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    pub const NOT_ADJACENT: &str = "NOT_ADJACENT";
    pub const LOADED: &str = "LOADED";
    pub const NOT_CARRYING: &str = "NOT_CARRYING";
    pub const NOT_ENOUGH_ENERGY: &str = "NOT_ENOUGH_ENERGY";
    pub const TARGET_FULL: &str = "TARGET_FULL";
    pub const BOX_NOT_FOUND: &str = "BOX_NOT_FOUND";
    pub const WRONG_GOODS: &str = "WRONG_GOODS";
    pub const INVALID_TARGET: &str = "INVALID_TARGET";
    pub const OUT_OF_BOUNDS: &str = "OUT_OF_BOUNDS";
    pub const CELL_BLOCKED: &str = "CELL_BLOCKED";
    pub const CELL_OCCUPIED: &str = "CELL_OCCUPIED";
    pub const NO_SUCH_OBJECT: &str = "NO_SUCH_OBJECT";
    pub const NO_PATH: &str = "NO_PATH";
    pub const CELL_CONTESTED: &str = "CELL_CONTESTED";
    pub const TARGET_CONTESTED: &str = "TARGET_CONTESTED";
    pub const CHAIN_BLOCKED: &str = "CHAIN_BLOCKED";
    pub const CHARGER_BUSY: &str = "CHARGER_BUSY";
    pub const TARGET_MOVED: &str = "TARGET_MOVED";
    pub const TARGET_GONE: &str = "TARGET_GONE";
    pub const NO_FUNDS: &str = "NO_FUNDS";
    pub const CREDIT_EXCEEDED: &str = "CREDIT_EXCEEDED";
    pub const ON_VEHICLE: &str = "ON_VEHICLE";
    pub const NO_FREE_DOCK: &str = "NO_FREE_DOCK";
    pub const NOT_EMPTY: &str = "NOT_EMPTY";
    pub const HAS_VEHICLE: &str = "HAS_VEHICLE";
    pub const ORDER_GONE: &str = "ORDER_GONE";
    pub const GOODS_MOVED: &str = "GOODS_MOVED";
    /// buy 购买装卸位：锚点不在边界墙上（或角格朝向无法唯一确定）。
    pub const NOT_ON_WALL: &str = "NOT_ON_WALL";
    /// 初始化阶段禁用动作与管理操作（docs/architecture/03 执行模型）。
    pub const INIT_PHASE: &str = "INIT_PHASE";

    /// 全表，供绑定层生成 Game.E。保持与 docs/game-design/08 一致。
    pub const ALL: &[(&str, &str)] = &[
        ("OK", OK),
        ("ARRIVED", ARRIVED),
        ("ALREADY_ACTED", ALREADY_ACTED),
        ("INVALID_ARGUMENT", INVALID_ARGUMENT),
        ("NOT_ADJACENT", NOT_ADJACENT),
        ("LOADED", LOADED),
        ("NOT_CARRYING", NOT_CARRYING),
        ("NOT_ENOUGH_ENERGY", NOT_ENOUGH_ENERGY),
        ("TARGET_FULL", TARGET_FULL),
        ("BOX_NOT_FOUND", BOX_NOT_FOUND),
        ("WRONG_GOODS", WRONG_GOODS),
        ("INVALID_TARGET", INVALID_TARGET),
        ("OUT_OF_BOUNDS", OUT_OF_BOUNDS),
        ("CELL_BLOCKED", CELL_BLOCKED),
        ("CELL_OCCUPIED", CELL_OCCUPIED),
        ("NO_SUCH_OBJECT", NO_SUCH_OBJECT),
        ("NO_PATH", NO_PATH),
        ("CELL_CONTESTED", CELL_CONTESTED),
        ("TARGET_CONTESTED", TARGET_CONTESTED),
        ("CHAIN_BLOCKED", CHAIN_BLOCKED),
        ("CHARGER_BUSY", CHARGER_BUSY),
        ("TARGET_MOVED", TARGET_MOVED),
        ("TARGET_GONE", TARGET_GONE),
        ("NO_FUNDS", NO_FUNDS),
        ("CREDIT_EXCEEDED", CREDIT_EXCEEDED),
        ("ON_VEHICLE", ON_VEHICLE),
        ("NO_FREE_DOCK", NO_FREE_DOCK),
        ("NOT_EMPTY", NOT_EMPTY),
        ("HAS_VEHICLE", HAS_VEHICLE),
        ("ORDER_GONE", ORDER_GONE),
        ("GOODS_MOVED", GOODS_MOVED),
        ("NOT_ON_WALL", NOT_ON_WALL),
        ("INIT_PHASE", INIT_PHASE),
    ];
}

// ---------------------------------------------------------------------------
// 实体（字段以 docs/game-design/08 对象与字段表为准）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Robot {
    pub id: Id,
    pub pos: Position,
    pub energy: u32,
    pub energy_max: u32,
    /// 携带货物 id；首期为单箱携带（None 即空载）。
    pub carry: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shelf {
    pub id: Id,
    pub pos: Position,
    pub box_ids: Vec<Id>,
    pub capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Charger {
    pub id: Id,
    pub pos: Position,
}

/// 装卸位：1×2 占地（宽一格、长两格）。锚点 `pos` 为靠墙缺口格，`ext` 为
/// 指向第二格（库内侧）的单位偏移向量；两格均为静态障碍，停靠货车跨越
/// 两格，机器人邻接第二格交互（docs/game-design/02）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dock {
    pub id: Id,
    pub pos: Position,
    pub ext: (i32, i32),
    pub docked_vehicle: Option<Id>,
    /// take 已预留、车辆尚未到场的订单。
    pub reserved_for: Option<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VehicleKind {
    /// 卖单车辆：对方卖出，送货来，待卸。
    In,
    /// 买单车辆：对方收购，来取货，待装。
    Out,
}

impl VehicleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            VehicleKind::In => "in",
            VehicleKind::Out => "out",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vehicle {
    pub id: Id,
    pub kind: VehicleKind,
    pub goods_type: String,
    /// 装卸交互格：所停靠装卸位的第二格（库内侧），机器人邻接此格装卸。
    pub interact_pos: Position,
    pub order_id: Id,
    pub box_ids: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundBox {
    pub id: Id,
    pub goods_type: String,
    pub pos: Position,
    /// 所在容器 id；地面为 None。
    pub holder: Option<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Sell,
    Buy,
}

impl OrderSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderSide::Sell => "sell",
            OrderSide::Buy => "buy",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: Id,
    pub side: OrderSide,
    pub goods_type: String,
    pub qty: u32,
    /// 单价（金币千分，i64 定点）。
    pub unit_price_milli: MilliGold,
    /// 已接订单：在场车辆与预留装卸位。
    pub vehicle: Option<Id>,
    pub dock: Option<Id>,
}

// ---------------------------------------------------------------------------
// Game.memory 线值（wire value）
// ---------------------------------------------------------------------------
// 受控数据树的跨进程表示。映射保存为有序键值对序列，不依赖 JSON Object
// 的键枚举规则（docs/architecture/06）。

/// 有限数字的可表示整数范围：[-(2^53-1), 2^53-1]。
pub const MAX_SAFE_INT: f64 = 9_007_199_254_740_991.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MemValue {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<MemValue>),
    Map(Vec<(String, MemValue)>),
}

impl MemValue {
    /// 权威值校验（docs/architecture/06）：
    /// 拒绝 NaN / Infinity、超出安全整数范围的整数值、超长字符串。
    /// 节点数 / 字节 / 深度上限由 memory 树在写入边界检查。
    pub fn validate(&self, max_string: usize) -> Result<(), String> {
        let mut err = None;
        self.walk(&mut |v| {
            if err.is_some() {
                return;
            }
            match v {
                MemValue::Num(n) => {
                    if !n.is_finite() {
                        err = Some(format!("数字必须有限，得到 {n}"));
                    } else if n.fract() == 0.0 && n.abs() > MAX_SAFE_INT {
                        err = Some(format!("整数超出安全范围 [-(2^53-1), 2^53-1]：{n}"));
                    }
                }
                MemValue::Str(s) if s.len() > max_string => {
                    err = Some(format!(
                        "字符串长度 {len} 超上限 {max_string}",
                        len = s.len()
                    ));
                }
                _ => {}
            }
        });
        err.map_or(Ok(()), Err)
    }

    fn walk(&self, f: &mut dyn FnMut(&MemValue)) {
        f(self);
        match self {
            MemValue::List(items) => {
                for it in items {
                    it.walk(f);
                }
            }
            MemValue::Map(pairs) => {
                for (_, v) in pairs {
                    v.walk(f);
                }
            }
            _ => {}
        }
    }

    /// 节点数（容器 + 标量叶）。
    pub fn node_count(&self) -> usize {
        let mut n = 0;
        self.walk(&mut |_| n += 1);
        n
    }

    /// 近似字节数：字符串 + 键 + 每节点固定开销。
    pub fn approx_bytes(&self) -> usize {
        const NODE_OVERHEAD: usize = 32;
        let mut b = 0;
        self.walk(&mut |v| {
            b += NODE_OVERHEAD;
            if let MemValue::Str(s) = v {
                b += s.len();
            }
        });
        if let MemValue::Map(pairs) = self {
            for (k, _) in pairs {
                b += k.len();
            }
        }
        b
    }

    pub fn depth(&self) -> usize {
        fn d(v: &MemValue) -> usize {
            match v {
                MemValue::List(items) => items.iter().map(d).max().map_or(1, |m| m + 1),
                MemValue::Map(pairs) => pairs.iter().map(|p| d(&p.1)).max().map_or(1, |m| m + 1),
                _ => 1,
            }
        }
        d(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_validation() {
        assert!(MemValue::Num(0.5).validate(1024).is_ok());
        assert!(MemValue::Num(MAX_SAFE_INT).validate(1024).is_ok());
        assert!(MemValue::Num(-MAX_SAFE_INT).validate(1024).is_ok());
        assert!(MemValue::Num(f64::NAN).validate(1024).is_err());
        assert!(MemValue::Num(f64::INFINITY).validate(1024).is_err());
        assert!(MemValue::Num(2f64.powi(53)).validate(1024).is_err());
    }

    #[test]
    fn string_limit() {
        assert!(MemValue::Str("x".repeat(5)).validate(4).is_err());
        assert!(MemValue::Str("x".repeat(4)).validate(4).is_ok());
    }

    #[test]
    fn shape_metrics() {
        let v = MemValue::Map(vec![
            ("a".into(), MemValue::Num(1.0)),
            ("b".into(), MemValue::List(vec![MemValue::Str("xy".into())])),
        ]);
        assert_eq!(v.node_count(), 4);
        assert_eq!(v.depth(), 3);
        assert!(v.approx_bytes() >= 32 * 4 + 2 + 2);
    }
}
