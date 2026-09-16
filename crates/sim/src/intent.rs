//! 意图与结算结果的数据类型（docs/game-design/03 受理规则、08 对象表）。

use ztw_model::Id;

/// take / give 的交互目标（受理时解析并固定；docs/game-design/08）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetRef {
    Shelf(Id),
    Vehicle(Id),
    Robot(Id),
}

impl TargetRef {
    pub fn id(&self) -> Id {
        match self {
            TargetRef::Shelf(id) | TargetRef::Vehicle(id) | TargetRef::Robot(id) => *id,
        }
    }
}

/// 已受理意图：每机器人每 tick 至多一条（docs/game-design/03 受理规则）。
/// box_id 在受理时解析固定（give / drop 缺省为当前携带物）。
#[derive(Debug, Clone)]
pub enum Intent {
    Move {
        robot_id: Id,
        dx: i32,
        dy: i32,
    },
    Charge {
        robot_id: Id,
        charger_id: Id,
    },
    Take {
        robot_id: Id,
        target: TargetRef,
        box_id: Id,
    },
    Give {
        robot_id: Id,
        target: TargetRef,
        box_id: Id,
    },
    Pick {
        robot_id: Id,
        x: i32,
        y: i32,
        box_id: Id,
    },
    Drop {
        robot_id: Id,
        x: i32,
        y: i32,
        box_id: Id,
    },
}

impl Intent {
    pub fn robot_id(&self) -> Id {
        match self {
            Intent::Move { robot_id, .. }
            | Intent::Charge { robot_id, .. }
            | Intent::Take { robot_id, .. }
            | Intent::Give { robot_id, .. }
            | Intent::Pick { robot_id, .. }
            | Intent::Drop { robot_id, .. } => *robot_id,
        }
    }
}

/// 上一 tick 结算结果（docs/game-design/03：成功为 OK；未受理动作时为 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct LastResult {
    pub action: &'static str,
    /// 动作参数（move 为方向名；take/give/charge 为目标 id；pick/drop 为 "x,y"）。
    pub arg: String,
    pub code: String,
}

/// 动作名固定码表（`LastResult.action` 的值域）。结算侧以同名字面量产生，
/// 快照恢复按此反查 `&'static str`；未知名视为存档损坏（值域冻结的
/// 一部分，新增动作名须同步此表并递增规则版本）。
pub const ACTION_NAMES: &[&str] = &["move", "charge", "take", "give", "pick", "drop"];
