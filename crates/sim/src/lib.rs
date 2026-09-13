//! tick 状态机、动作受理与统一结算（受理 → 只读求解 → 原子提交）、固定挂单
//! 市场与车辆生命周期。语义以 docs/game-design/03、04 为准，结构以
//! docs/architecture/02-simulation-core.md「结算三段式」为准。
//!
//! B1 范围（docs/architecture/08 原型 B 的模拟核心）：move 之外补全 charge /
//! take / give / pick / drop 与机器人间被动转交；车辆离场 → 订单完成 → 收款 →
//! 装卸位释放闭环；market.cancel 与最小 manage_destroy；xoshiro 分流 PRNG
//! 承担装卸位分配。市场挂单刷新与价格波动、购买类管理操作、find_path 属
//! 后续里程碑。

use std::collections::{BTreeMap, BTreeSet};

use ztw_model::{
    Charger, GroundBox, Id, MilliGold, Order, OrderSide, Port, Position, Robot, Shelf, Vehicle,
    VehicleKind, codes,
};

// ---------------------------------------------------------------------------
// 数值锚点（临时；docs/game-design/07 列为随原型平衡的待定项）
// ---------------------------------------------------------------------------

/// 每 tick 充电量（封顶 energy_max）。
pub const CHARGE_PER_TICK: u32 = 25;
/// 动作耗电（移动不耗电；四者数值不同是文档要求）。
pub const ENERGY_COST_TAKE: u32 = 6;
pub const ENERGY_COST_GIVE: u32 = 5;
pub const ENERGY_COST_PICK: u32 = 4;
pub const ENERGY_COST_DROP: u32 = 3;
/// 取消手续费 = 订单额 × 10%（向下取整，退款下限 0；docs/game-design/04）。
pub const CANCEL_FEE_NUMERATOR: MilliGold = 1;
pub const CANCEL_FEE_DENOMINATOR: MilliGold = 10;
/// 销毁退款 = 设备价 × 50%；设备价取 docs/game-design/09 锚点（千分金币）。
pub const DESTROY_REFUND_NUMERATOR: MilliGold = 1;
pub const DESTROY_REFUND_DENOMINATOR: MilliGold = 2;
pub const PRICE_ROBOT: MilliGold = 650_000;
pub const PRICE_SHELF: MilliGold = 175_000;
pub const PRICE_CHARGER: MilliGold = 300_000;
pub const PRICE_PORT: MilliGold = 500_000;

// ---------------------------------------------------------------------------
// PRNG（docs/architecture/02 确定性规则：固定种子、按子系统独立分流）
// ---------------------------------------------------------------------------

/// xoshiro256**：模拟专用固定种子 PRNG。各流自世界种子 + 固定子系统标识
/// 派生，状态分别入存档；状态字节布局随存档格式冻结，变更须迁移旧档。
#[derive(Debug, Clone, PartialEq)]
pub struct Xoshiro256 {
    s: [u64; 4],
}

impl Xoshiro256 {
    /// 自世界种子与子系统标识派生（SplitMix64 混合的标准派生路径）。
    pub fn derive(world_seed: u64, subsystem: &str) -> Xoshiro256 {
        fn splitmix64(x: &mut u64) -> u64 {
            *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        let mut x = world_seed;
        for b in subsystem.bytes() {
            splitmix64(&mut x);
            x ^= b as u64;
        }
        let mut s = [
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
        ];
        if s == [0, 0, 0, 0] {
            s[0] = 1; // 全零是 xoshiro 退化态，防御
        }
        Xoshiro256 { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// [0, n) 取整。装卸口数量是个位数，模偏差可忽略（记录在案）。
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// 状态字（state_hash 与存档用）。
    pub fn state_words(&self) -> [u64; 4] {
        self.s
    }
}

// ---------------------------------------------------------------------------
// 意图与目标
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone)]
struct PendingArrival {
    order_id: Id,
    arrive_tick: u64,
}

#[derive(Debug, Clone)]
pub struct World {
    pub tick: u64,
    pub map_w: i32,
    pub map_h: i32,
    /// 静态障碍之外的墙（测试用）；货架 / 充电桩 / 装卸口 / 地面货物另行计入。
    walls: BTreeSet<Position>,
    /// 世界种子：一切子 PRNG 流的派生根。
    pub seed: u64,
    /// 装卸位分配流（docs/architecture/02：市场、装卸位分配、场景独立分流）。
    rng_port: Xoshiro256,
    pub next_id: Id,
    pub gold_milli: MilliGold,
    pub debt_milli: MilliGold,
    pub robots: BTreeMap<Id, Robot>,
    pub shelves: BTreeMap<Id, Shelf>,
    pub chargers: BTreeMap<Id, Charger>,
    pub ports: BTreeMap<Id, Port>,
    pub vehicles: BTreeMap<Id, Vehicle>,
    pub ground_boxes: BTreeMap<Id, GroundBox>,
    /// 市场挂单（固定挂单；刷新与价格波动属里程碑 3）。
    pub listings: BTreeMap<Id, Order>,
    /// 已接未完成订单。
    pub my_orders: BTreeMap<Id, Order>,
    /// take 排定的车辆到场事件（tick 边界、loop() 之前生效）。
    arrivals: Vec<PendingArrival>,
    /// 本 tick 已受理意图。
    intents: Vec<Intent>,
    /// 上一 tick 结算结果，按机器人 id。
    pub last_results: BTreeMap<Id, LastResult>,
}

impl World {
    pub fn new_empty(map_w: i32, map_h: i32, gold_milli: MilliGold) -> World {
        World {
            tick: 0,
            map_w,
            map_h,
            walls: BTreeSet::new(),
            seed: 0,
            rng_port: Xoshiro256::derive(0, "port"),
            next_id: 1,
            gold_milli,
            debt_milli: 0,
            robots: BTreeMap::new(),
            shelves: BTreeMap::new(),
            chargers: BTreeMap::new(),
            ports: BTreeMap::new(),
            vehicles: BTreeMap::new(),
            ground_boxes: BTreeMap::new(),
            listings: BTreeMap::new(),
            my_orders: BTreeMap::new(),
            arrivals: Vec::new(),
            intents: Vec::new(),
            last_results: BTreeMap::new(),
        }
    }

    /// 设定世界种子（须在世界构造前调用，派生流随之重建）。
    pub fn with_seed(mut self, seed: u64) -> World {
        self.seed = seed;
        self.rng_port = Xoshiro256::derive(seed, "port");
        self
    }

    fn alloc_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_wall(&mut self, p: Position) {
        self.walls.insert(p);
    }

    pub fn add_robot(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.robots.insert(
            id,
            Robot {
                id,
                pos,
                energy: 100,
                energy_max: 100,
                carry: None,
            },
        );
        id
    }

    pub fn add_shelf(&mut self, pos: Position) -> Id {
        self.add_shelf_sized(pos, 4)
    }

    /// 指定容量的货架（测试与场景构造用）。
    pub fn add_shelf_sized(&mut self, pos: Position, capacity: usize) -> Id {
        let id = self.alloc_id();
        self.shelves.insert(
            id,
            Shelf {
                id,
                pos,
                box_ids: Vec::new(),
                capacity,
            },
        );
        id
    }

    pub fn add_charger(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.chargers.insert(id, Charger { id, pos });
        id
    }

    pub fn add_port(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.ports.insert(
            id,
            Port {
                id,
                pos,
                docked_vehicle: None,
                reserved_for: None,
            },
        );
        id
    }

    /// 直接放置地面货物（测试与场景构造用；每格至多一箱由调用方保证）。
    pub fn add_ground_box(&mut self, goods_type: &str, pos: Position) -> Id {
        let id = self.alloc_id();
        self.ground_boxes.insert(
            id,
            GroundBox {
                id,
                goods_type: goods_type.to_string(),
                pos,
                holder: None,
            },
        );
        id
    }

    pub fn add_listing(
        &mut self,
        side: OrderSide,
        goods_type: &str,
        qty: u32,
        unit_price_milli: MilliGold,
    ) -> Id {
        let id = self.alloc_id();
        self.listings.insert(
            id,
            Order {
                id,
                side,
                goods_type: goods_type.to_string(),
                qty,
                unit_price_milli,
                vehicle: None,
                port: None,
            },
        );
        id
    }

    /// 静态可通行判定：界内且非墙、非货架、非充电桩、非装卸口、非地面货物。
    /// 车辆停靠在装卸口格上（口本身已计入障碍），机器人不是静态障碍。
    pub fn statically_passable(&self, p: Position) -> bool {
        if p.x < 0 || p.y < 0 || p.x >= self.map_w || p.y >= self.map_h {
            return false;
        }
        if self.walls.contains(&p) {
            return false;
        }
        if self.shelves.values().any(|s| s.pos == p) {
            return false;
        }
        if self.chargers.values().any(|c| c.pos == p) {
            return false;
        }
        if self.ports.values().any(|p0| p0.pos == p) {
            return false;
        }
        if self
            .ground_boxes
            .values()
            .any(|b| b.holder.is_none() && b.pos == p)
        {
            return false;
        }
        true
    }

    /// tick 开始时占位该格的机器人（若有）。
    pub fn robot_at(&self, p: Position) -> Option<Id> {
        self.robots.values().find(|r| r.pos == p).map(|r| r.id)
    }

    /// 该地面格上的无主货物（每格至多一箱由受理与结算维护）。
    fn ground_box_at(&self, p: Position) -> Option<Id> {
        self.ground_boxes
            .values()
            .find(|b| b.holder.is_none() && b.pos == p)
            .map(|b| b.id)
    }

    // -----------------------------------------------------------------------
    // 阶段 1：tick 边界事件（车辆到场；市场刷新属里程碑 3）
    // -----------------------------------------------------------------------

    pub fn boundary_events(&mut self) {
        let mut due: Vec<Id> = self
            .arrivals
            .iter()
            .filter(|a| a.arrive_tick == self.tick)
            .map(|a| a.order_id)
            .collect();
        due.sort_unstable(); // 同边界多辆车：按订单 id 升序到场（遍历序规则）
        self.arrivals.retain(|a| a.arrive_tick > self.tick);
        for order_id in due {
            self.spawn_vehicle(order_id);
        }
    }

    fn spawn_vehicle(&mut self, order_id: Id) {
        let Some(order) = self.my_orders.get(&order_id) else {
            return;
        };
        let kind = match order.side {
            OrderSide::Sell => VehicleKind::In,
            OrderSide::Buy => VehicleKind::Out,
        };
        let port_id = order.port.expect("已接订单必有预留装卸口");
        let port_pos = self.ports.get(&port_id).expect("装卸口存在").pos;
        let goods = order.goods_type.clone();
        let qty = order.qty;
        let vehicle_id = self.alloc_id();
        let mut box_ids = Vec::new();
        if kind == VehicleKind::In {
            // 卖单车辆带货到场（固定流程：按订单数量生成货物）。
            for _ in 0..qty {
                let box_id = self.alloc_id();
                self.ground_boxes.insert(
                    box_id,
                    GroundBox {
                        id: box_id,
                        goods_type: goods.clone(),
                        pos: port_pos,
                        holder: Some(vehicle_id),
                    },
                );
                box_ids.push(box_id);
            }
        }
        self.vehicles.insert(
            vehicle_id,
            Vehicle {
                id: vehicle_id,
                kind,
                goods_type: goods,
                interact_pos: port_pos,
                order_id,
                box_ids,
            },
        );
        if let Some(port) = self.ports.get_mut(&port_id) {
            port.docked_vehicle = Some(vehicle_id);
            port.reserved_for = None;
        }
        if let Some(order) = self.my_orders.get_mut(&order_id) {
            order.vehicle = Some(vehicle_id);
        }
    }

    // -----------------------------------------------------------------------
    // 动作受理（静态检查，基于调用时世界状态 = tick 开始快照 + 已生效管理
    // 操作；docs/game-design/03：只判静态条件，同 tick 冲突留给结算）
    // -----------------------------------------------------------------------

    /// 受理前公共检查：机器人存在且本 tick 未行动。
    fn actor(&self, robot_id: Id) -> Result<&Robot, &'static str> {
        let Some(robot) = self.robots.get(&robot_id) else {
            return Err(codes::NO_SUCH_OBJECT);
        };
        if self.intents.iter().any(|i| i.robot_id() == robot_id) {
            return Err(codes::ALREADY_ACTED);
        }
        Ok(robot)
    }

    /// 解析交互目标：货架 / 车辆 / 机器人之外（含不存在）报错。
    fn resolve_target(&self, target_id: Id) -> Result<TargetRef, &'static str> {
        if self.shelves.contains_key(&target_id) {
            Ok(TargetRef::Shelf(target_id))
        } else if self.vehicles.contains_key(&target_id) {
            Ok(TargetRef::Vehicle(target_id))
        } else if self.robots.contains_key(&target_id) {
            Ok(TargetRef::Robot(target_id))
        } else if self.chargers.contains_key(&target_id)
            || self.ports.contains_key(&target_id)
            || self.ground_boxes.contains_key(&target_id)
        {
            Err(codes::INVALID_TARGET)
        } else {
            Err(codes::NO_SUCH_OBJECT)
        }
    }

    /// 交互格：车辆为 interact_pos，其余为所在格（docs/game-design/08）。
    fn target_pos(&self, t: TargetRef) -> Position {
        match t {
            TargetRef::Shelf(id) => self.shelves[&id].pos,
            TargetRef::Vehicle(id) => self.vehicles[&id].interact_pos,
            TargetRef::Robot(id) => self.robots[&id].pos,
        }
    }

    fn target_alive(&self, t: TargetRef) -> bool {
        match t {
            TargetRef::Shelf(id) => self.shelves.contains_key(&id),
            TargetRef::Vehicle(id) => self.vehicles.contains_key(&id),
            TargetRef::Robot(id) => self.robots.contains_key(&id),
        }
    }

    /// 目标当前是否持有该货物。
    fn target_holds(&self, t: TargetRef, box_id: Id) -> bool {
        match t {
            TargetRef::Shelf(id) => self.shelves[&id].box_ids.contains(&box_id),
            TargetRef::Vehicle(id) => self.vehicles[&id].box_ids.contains(&box_id),
            TargetRef::Robot(id) => self.robots[&id].carry == Some(box_id),
        }
    }

    /// 出库车的装载容量即订单数量（到场空车，装满即离场）。
    fn vehicle_capacity(&self, vehicle_id: Id) -> usize {
        let v = &self.vehicles[&vehicle_id];
        self.my_orders
            .get(&v.order_id)
            .map(|o| o.qty as usize)
            .unwrap_or(v.box_ids.len())
    }

    /// move 受理：只判定调用时可知的静态条件。目标格争抢、链受阻等
    /// 依赖同 tick 其他意图的冲突一律留给结算。
    pub fn accept_move(&mut self, robot_id: Id, dx: i32, dy: i32) -> &'static str {
        if !(dx == 0 || dy == 0) || dx.abs() > 1 || dy.abs() > 1 || (dx == 0 && dy == 0) {
            return codes::INVALID_ARGUMENT;
        }
        let pos = match self.actor(robot_id) {
            Ok(r) => r.pos,
            Err(code) => return code,
        };
        let target = pos.step(dx, dy);
        if target.x < 0 || target.y < 0 || target.x >= self.map_w || target.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !self.statically_passable(target) {
            return codes::CELL_BLOCKED;
        }
        self.intents.push(Intent::Move { robot_id, dx, dy });
        codes::OK
    }

    /// charge 受理：无参动作——受理时按相邻 id 最小桩固定目标，之后不换桩
    /// （docs/game-design/03 电力与充电）。同 tick 竞争留给结算（CHARGER_BUSY）。
    pub fn accept_charge(&mut self, robot_id: Id) -> &'static str {
        let pos = match self.actor(robot_id) {
            Ok(r) => r.pos,
            Err(code) => return code,
        };
        let Some(charger_id) = self
            .chargers
            .values()
            .filter(|c| c.pos.adjacent(&pos))
            .map(|c| c.id)
            .min()
        else {
            return codes::NOT_ADJACENT;
        };
        self.intents.push(Intent::Charge {
            robot_id,
            charger_id,
        });
        codes::OK
    }

    /// take 受理：从相邻容器取得指定货物；自身须空载。
    pub fn accept_take(&mut self, robot_id: Id, target_id: Id, box_id: Id) -> &'static str {
        let (pos, energy, loaded) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry.is_some()),
            Err(code) => return code,
        };
        if loaded {
            return codes::LOADED;
        }
        let target = match self.resolve_target(target_id) {
            Ok(t) => t,
            Err(code) => return code,
        };
        if !self.target_pos(target).adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        if !self.target_holds(target, box_id) {
            return codes::BOX_NOT_FOUND;
        }
        if energy < ENERGY_COST_TAKE {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Take {
            robot_id,
            target,
            box_id,
        });
        codes::OK
    }

    /// give 受理：将携带物放入相邻容器（货架 / 车辆 / 空载机器人）。
    /// box_id 缺省为当前携带物（为多箱预留）。同 tick 容量争抢留给结算。
    pub fn accept_give(&mut self, robot_id: Id, target_id: Id, box_id: Option<Id>) -> &'static str {
        let (pos, energy, carry) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry),
            Err(code) => return code,
        };
        let Some(carried) = carry else {
            return codes::NOT_CARRYING;
        };
        let box_id = box_id.unwrap_or(carried);
        if carry != Some(box_id) {
            return codes::BOX_NOT_FOUND; // 指定货物不在自身携带
        }
        let target = match self.resolve_target(target_id) {
            Ok(t) => t,
            Err(code) => return code,
        };
        if !self.target_pos(target).adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        match target {
            TargetRef::Shelf(shelf_id) => {
                let s = &self.shelves[&shelf_id];
                if s.box_ids.len() >= s.capacity {
                    return codes::TARGET_FULL;
                }
            }
            TargetRef::Vehicle(vehicle_id) => {
                let v = &self.vehicles[&vehicle_id];
                if self.ground_boxes[&box_id].goods_type != v.goods_type {
                    return codes::WRONG_GOODS;
                }
                if v.box_ids.len() >= self.vehicle_capacity(vehicle_id) {
                    return codes::TARGET_FULL;
                }
            }
            TargetRef::Robot(target_robot) => {
                if self.robots[&target_robot].carry.is_some() {
                    return codes::TARGET_FULL; // 接收方非空载
                }
            }
        }
        if energy < ENERGY_COST_GIVE {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Give {
            robot_id,
            target,
            box_id,
        });
        codes::OK
    }

    /// pick 受理：拾起相邻地面格的货物；自身须空载。
    pub fn accept_pick(&mut self, robot_id: Id, x: i32, y: i32) -> &'static str {
        let (pos, energy, loaded) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry.is_some()),
            Err(code) => return code,
        };
        if loaded {
            return codes::LOADED;
        }
        let cell = Position::new(x, y);
        if cell.x < 0 || cell.y < 0 || cell.x >= self.map_w || cell.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !cell.adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        let Some(box_id) = self.ground_box_at(cell) else {
            return codes::BOX_NOT_FOUND;
        };
        if energy < ENERGY_COST_PICK {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Pick {
            robot_id,
            x,
            y,
            box_id,
        });
        codes::OK
    }

    /// drop 受理：将携带物放到相邻空地面格。目标格被机器人占用不在受理
    /// 判定：能否放下取决于其本 tick 是否成功离开（结算判 CELL_OCCUPIED）。
    pub fn accept_drop(
        &mut self,
        robot_id: Id,
        x: i32,
        y: i32,
        box_id: Option<Id>,
    ) -> &'static str {
        let (pos, energy, carry) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry),
            Err(code) => return code,
        };
        let Some(carried) = carry else {
            return codes::NOT_CARRYING;
        };
        let box_id = box_id.unwrap_or(carried);
        if carry != Some(box_id) {
            return codes::BOX_NOT_FOUND;
        }
        let cell = Position::new(x, y);
        if cell.x < 0 || cell.y < 0 || cell.x >= self.map_w || cell.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !cell.adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        // 地面货物占格优先报 CELL_OCCUPIED（docs/game-design/08 码表），
        // 其余静态障碍（墙 / 货架 / 桩 / 口）报 CELL_BLOCKED。
        if self.ground_box_at(cell).is_some() {
            return codes::CELL_OCCUPIED;
        }
        if !self.statically_passable(cell) {
            return codes::CELL_BLOCKED;
        }
        if energy < ENERGY_COST_DROP {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Drop {
            robot_id,
            x,
            y,
            box_id,
        });
        codes::OK
    }

    // -----------------------------------------------------------------------
    // 管理操作（即时生效；docs/game-design/08）：take / cancel / destroy
    // -----------------------------------------------------------------------

    /// 接单：校验并预留空闲装卸位；卖单（玩家买入）即时扣款；车辆下一 tick
    /// 边界到场。空闲口由装卸位分配流随机占用（docs/game-design/04）。
    pub fn manage_take(&mut self, order_id: Id) -> (&'static str, Option<TakeEffect>) {
        let Some(order) = self.listings.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let unit_price = order.unit_price_milli;
        let mut free_ports: Vec<Id> = self
            .ports
            .values()
            .filter(|p| p.docked_vehicle.is_none() && p.reserved_for.is_none())
            .map(|p| p.id)
            .collect();
        free_ports.sort_unstable(); // 遍历序规则：按 id 升序后抽样
        if free_ports.is_empty() {
            return (codes::NO_FREE_PORT, None);
        }
        let idx = self.rng_port.below(free_ports.len());
        let port_id = free_ports[idx];
        // 卖单（玩家买入）需即时扣款：成本只算一次，校验与扣款同源。
        let cost = match side {
            OrderSide::Sell => (qty as i64).checked_mul(unit_price),
            OrderSide::Buy => Some(0),
        };
        let cost = match cost {
            Some(c) if c <= self.gold_milli || side == OrderSide::Buy => c,
            _ => return (codes::NO_FUNDS, None),
        };
        let order = self.listings.remove(&order_id).expect("已校验存在");
        if side == OrderSide::Sell {
            self.gold_milli -= cost;
        }
        self.ports.get_mut(&port_id).expect("存在").reserved_for = Some(order_id);
        let mut taken = order;
        taken.port = Some(port_id);
        self.my_orders.insert(order_id, taken.clone());
        self.arrivals.push(PendingArrival {
            order_id,
            arrive_tick: self.tick + 1,
        });
        let effect = TakeEffect {
            order_id,
            port_id,
            order: taken,
        };
        (codes::OK, Some(effect))
    }

    /// 取消订单：车辆须恢复到场初态（入库车满、出库车空；同类型同数量即可，
    /// 未到场即初态）。卖单（买入）退款扣手续费；买单（卖出）手续费扣金币、
    /// 不足自动计欠款。取消的订单不回市场（docs/game-design/04）。
    pub fn manage_cancel(&mut self, order_id: Id) -> (&'static str, Option<CancelEffect>) {
        let Some(order) = self.my_orders.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let amount = (qty as i64).saturating_mul(order.unit_price_milli);
        let vehicle_id = order.vehicle;
        let port_id = order.port;
        let fee = amount / CANCEL_FEE_DENOMINATOR * CANCEL_FEE_NUMERATOR;
        // 车辆到场后校验可恢复性；未到场（arrival 待定）即初态。
        if let Some(vid) = vehicle_id {
            let Some(v) = self.vehicles.get(&vid) else {
                return (codes::ORDER_GONE, None); // 理论不可达：在场车辆必在
            };
            let restorable = match v.kind {
                VehicleKind::In => v.box_ids.len() as u32 == qty,
                VehicleKind::Out => v.box_ids.is_empty(),
            };
            if !restorable {
                return (codes::GOODS_MOVED, None);
            }
        }
        let (refund, gold_pay, debt_add) = match side {
            OrderSide::Sell => ((amount - fee).max(0), 0, 0), // 退款下限 0
            OrderSide::Buy => {
                let pay = fee.min(self.gold_milli.max(0));
                (0, pay, fee - pay) // 手续费扣金币、不足计欠款
            }
        };
        self.gold_milli += refund - gold_pay;
        self.debt_milli += debt_add;
        // 车辆与其上货物退回对方；未到场则撤销到场事件。
        if let Some(vid) = vehicle_id {
            if let Some(v) = self.vehicles.remove(&vid) {
                for b in &v.box_ids {
                    self.ground_boxes.remove(b);
                }
            }
        } else {
            self.arrivals.retain(|a| a.order_id != order_id);
        }
        if let Some(pid) = port_id
            && let Some(p) = self.ports.get_mut(&pid)
        {
            if p.docked_vehicle == vehicle_id {
                p.docked_vehicle = None;
            }
            if p.reserved_for == Some(order_id) {
                p.reserved_for = None;
            }
        }
        self.my_orders.remove(&order_id);
        (
            codes::OK,
            Some(CancelEffect {
                order_id,
                vehicle_id,
                port_id,
                refund_milli: refund,
                fee_milli: fee,
            }),
        )
    }

    /// 最小销毁管理操作（docs/architecture/08 原型 B 回归第 5 条所需）。
    /// 退款为设备价一半（临时锚点）；车辆不可直接销毁（cancel 是唯一路径）；
    /// 买入货物卸离车辆前不可销毁（docs/game-design/04）。
    pub fn manage_destroy(&mut self, target_id: Id) -> (&'static str, Option<DestroyEffect>) {
        if let Some(r) = self.robots.get(&target_id) {
            if r.carry.is_some() {
                return (codes::NOT_EMPTY, None);
            }
            let pos = r.pos;
            self.robots.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Robot, target_id, Some(pos), PRICE_ROBOT);
        }
        if let Some(s) = self.shelves.get(&target_id) {
            if !s.box_ids.is_empty() {
                return (codes::NOT_EMPTY, None);
            }
            let pos = s.pos;
            self.shelves.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Shelf, target_id, Some(pos), PRICE_SHELF);
        }
        if let Some(c) = self.chargers.get(&target_id) {
            let pos = c.pos;
            self.chargers.remove(&target_id);
            return self.finish_destroy(
                DestroyedKind::Charger,
                target_id,
                Some(pos),
                PRICE_CHARGER,
            );
        }
        if let Some(p) = self.ports.get(&target_id) {
            if p.docked_vehicle.is_some() || p.reserved_for.is_some() {
                return (codes::HAS_VEHICLE, None);
            }
            let pos = p.pos;
            self.ports.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Port, target_id, Some(pos), PRICE_PORT);
        }
        if let Some(b) = self.ground_boxes.get(&target_id) {
            let holder = b.holder;
            let pos = b.pos;
            if holder.is_some_and(|h| self.vehicles.contains_key(&h)) {
                return (codes::ON_VEHICLE, None);
            }
            self.ground_boxes.remove(&target_id);
            let freed = if holder.is_none() { Some(pos) } else { None };
            if let Some(h) = holder {
                if let Some(r) = self.robots.get_mut(&h) {
                    if r.carry == Some(target_id) {
                        r.carry = None;
                    }
                } else if let Some(s) = self.shelves.get_mut(&h) {
                    s.box_ids.retain(|b| b != &target_id);
                }
            }
            // 货物无设备价，销毁止损无退款。
            return self.finish_destroy(DestroyedKind::GroundBox, target_id, freed, 0);
        }
        if self.vehicles.contains_key(&target_id) {
            return (codes::INVALID_TARGET, None);
        }
        (codes::NO_SUCH_OBJECT, None)
    }

    fn finish_destroy(
        &mut self,
        kind: DestroyedKind,
        target_id: Id,
        freed_cell: Option<Position>,
        price: MilliGold,
    ) -> (&'static str, Option<DestroyEffect>) {
        let refund = price / DESTROY_REFUND_DENOMINATOR * DESTROY_REFUND_NUMERATOR;
        self.gold_milli += refund;
        (
            codes::OK,
            Some(DestroyEffect {
                kind,
                target_id,
                refund_milli: refund,
                freed_cell,
            }),
        )
    }

    // -----------------------------------------------------------------------
    // 阶段 4：统一结算（受理 → 只读求解 → 原子提交）
    // -----------------------------------------------------------------------

    /// 结算本 tick 已受理意图。求解是「最终管理状态 + 意图集合 → 计划」的
    /// 纯函数：任意受理顺序得到同一计划（排列测试覆盖）。
    pub fn settle(&mut self) -> BTreeMap<Id, LastResult> {
        let plan = self.solve();
        self.commit(plan)
    }

    /// 只读求解，产出完整结算计划。次序固化（docs/game-design/03 结算资源
    /// 与执行顺序）：① 管理终态重校验 → ② move/drop 联合落点候选与移动
    /// 依赖 → ③ 转交目标移动判定 → ④ 剩余动作按机器人 id 升序原子占用
    /// 资源 → ⑤ 车辆离场。电量在受理时按快照校验且结算前不变，无需重查
    /// （“不预支同 tick 充入电量”由此成立）。
    fn solve(&self) -> SettlementPlan {
        let mut plan = SettlementPlan::default();
        #[derive(Clone, Copy)]
        struct MoveCand {
            robot: Id,
            from: Position,
            to: Position,
        }
        #[derive(Clone, Copy)]
        struct DropCand {
            robot: Id,
            cell: Position,
            box_id: Id,
        }
        // 资源裁决阶段的动作（drop 落点胜出者并入）。
        enum Res {
            Charge {
                robot: Id,
                charger: Id,
            },
            Take {
                robot: Id,
                target: TargetRef,
                box_id: Id,
            },
            Give {
                robot: Id,
                target: TargetRef,
                box_id: Id,
            },
            Pick {
                robot: Id,
                cell: Position,
                box_id: Id,
            },
            Drop {
                robot: Id,
                cell: Position,
                box_id: Id,
            },
        }
        impl Res {
            fn robot(&self) -> Id {
                match self {
                    Res::Charge { robot, .. }
                    | Res::Take { robot, .. }
                    | Res::Give { robot, .. }
                    | Res::Pick { robot, .. }
                    | Res::Drop { robot, .. } => *robot,
                }
            }
        }

        // ① 管理终态重校验：主体 / 目标被删或静态条件失效的意图在此出局，
        // 存活者按类型分拣。
        let mut move_cands: Vec<MoveCand> = Vec::new();
        let mut drop_cands: Vec<DropCand> = Vec::new();
        let mut res_acts: Vec<Res> = Vec::new();
        for intent in &self.intents {
            let robot_id = intent.robot_id();
            let Some(robot) = self.robots.get(&robot_id) else {
                // 主体被管理操作删除：结果保留在诊断记录（镜像不展示）。
                let (action, arg) = intent_label(intent);
                record(&mut plan, robot_id, action, arg, codes::TARGET_GONE);
                continue;
            };
            match intent {
                Intent::Move { dx, dy, .. } => {
                    let to = robot.pos.step(*dx, *dy);
                    let code = if to.x < 0 || to.y < 0 || to.x >= self.map_w || to.y >= self.map_h {
                        codes::OUT_OF_BOUNDS
                    } else if !self.statically_passable(to) {
                        codes::CELL_BLOCKED
                    } else {
                        move_cands.push(MoveCand {
                            robot: robot_id,
                            from: robot.pos,
                            to,
                        });
                        codes::OK
                    };
                    if code != codes::OK {
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, code);
                    }
                }
                Intent::Charge { charger_id, .. } => {
                    if !self.chargers.contains_key(charger_id) {
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, codes::TARGET_GONE);
                    } else {
                        res_acts.push(Res::Charge {
                            robot: robot_id,
                            charger: *charger_id,
                        });
                    }
                }
                Intent::Take { target, box_id, .. } => {
                    let holds = self.ground_boxes.contains_key(box_id)
                        && self.target_holds(*target, *box_id);
                    if holds {
                        res_acts.push(Res::Take {
                            robot: robot_id,
                            target: *target,
                            box_id: *box_id,
                        });
                    } else {
                        let code = if !self.target_alive(*target) {
                            codes::TARGET_GONE
                        } else {
                            codes::BOX_NOT_FOUND
                        };
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, code);
                    }
                }
                Intent::Give { target, box_id, .. } => {
                    let carrying =
                        robot.carry == Some(*box_id) && self.ground_boxes.contains_key(box_id);
                    if carrying && self.target_alive(*target) {
                        res_acts.push(Res::Give {
                            robot: robot_id,
                            target: *target,
                            box_id: *box_id,
                        });
                    } else {
                        let code = if !carrying {
                            codes::BOX_NOT_FOUND
                        } else {
                            codes::TARGET_GONE
                        };
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, code);
                    }
                }
                Intent::Pick { x, y, box_id, .. } => {
                    let still = self
                        .ground_boxes
                        .get(box_id)
                        .is_some_and(|b| b.holder.is_none() && b.pos == Position::new(*x, *y));
                    if still {
                        res_acts.push(Res::Pick {
                            robot: robot_id,
                            cell: Position::new(*x, *y),
                            box_id: *box_id,
                        });
                    } else {
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, codes::BOX_NOT_FOUND);
                    }
                }
                Intent::Drop { x, y, box_id, .. } => {
                    let cell = Position::new(*x, *y);
                    let carrying =
                        robot.carry == Some(*box_id) && self.ground_boxes.contains_key(box_id);
                    let code = if !carrying {
                        codes::BOX_NOT_FOUND
                    } else if cell.x < 0
                        || cell.y < 0
                        || cell.x >= self.map_w
                        || cell.y >= self.map_h
                    {
                        codes::OUT_OF_BOUNDS
                    } else if self.ground_box_at(cell).is_some() {
                        codes::CELL_OCCUPIED
                    } else if !self.statically_passable(cell) {
                        codes::CELL_BLOCKED
                    } else {
                        drop_cands.push(DropCand {
                            robot: robot_id,
                            cell,
                            box_id: *box_id,
                        });
                        codes::OK
                    };
                    if code != codes::OK {
                        let (action, arg) = intent_label(intent);
                        record(&mut plan, robot_id, action, arg, code);
                    }
                }
            }
        }

        // ② move 与 drop 的联合落点候选：同格争抢，机器人 id 小者胜，
        // 候选失败不递补（败者 CELL_CONTESTED）。
        #[derive(Clone, Copy)]
        enum Landing {
            M(MoveCand),
            D(DropCand),
        }
        let mut by_cell: BTreeMap<Position, Vec<Landing>> = BTreeMap::new();
        for m in move_cands {
            by_cell.entry(m.to).or_default().push(Landing::M(m));
        }
        for d in drop_cands {
            by_cell.entry(d.cell).or_default().push(Landing::D(d));
        }
        let mut move_winners: Vec<MoveCand> = Vec::new();
        let mut drop_winners: Vec<DropCand> = Vec::new();
        for (_cell, mut cands) in by_cell {
            cands.sort_by_key(|l| match l {
                Landing::M(m) => m.robot,
                Landing::D(d) => d.robot,
            });
            match cands.remove(0) {
                Landing::M(m) => move_winners.push(m),
                Landing::D(d) => drop_winners.push(d),
            }
            for loser in cands {
                match loser {
                    Landing::M(m) => record(
                        &mut plan,
                        m.robot,
                        "move",
                        dir_name(m.to.x - m.from.x, m.to.y - m.from.y).to_string(),
                        codes::CELL_CONTESTED,
                    ),
                    Landing::D(d) => record(
                        &mut plan,
                        d.robot,
                        "drop",
                        format!("{},{}", d.cell.x, d.cell.y),
                        codes::CELL_CONTESTED,
                    ),
                }
            }
        }
        move_winners.sort_by_key(|m| m.robot);
        drop_winners.sort_by_key(|d| d.robot);

        // 移动依赖求解（只读推演）：成功 = 目标格 tick 开始无机器人，或占位
        // 者的候选移动成功离开。二元交换是长度 2 的依赖环 → CHAIN_BLOCKED；
        // 长度 ≥3 的环允许。迭代传播失败直到不动点（候选失败不递补）。
        let mut move_ok: BTreeMap<Id, bool> =
            move_winners.iter().map(|m| (m.robot, true)).collect();
        loop {
            let mut changed = false;
            for m in &move_winners {
                if !move_ok[&m.robot] {
                    continue;
                }
                let blocked = match self.robot_at(m.to) {
                    None => false,
                    Some(occ) => occ != m.robot && !move_ok.get(&occ).copied().unwrap_or(false),
                };
                if blocked {
                    move_ok.insert(m.robot, false);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // 二元交换显式判负：不动点初值全 true 时互相依赖不会自行传播。
        for i in 0..move_winners.len() {
            for j in (i + 1)..move_winners.len() {
                let (a, b) = (&move_winners[i], &move_winners[j]);
                if a.from == b.to && b.from == a.to {
                    move_ok.insert(a.robot, false);
                    move_ok.insert(b.robot, false);
                }
            }
        }
        for m in &move_winners {
            let arg = dir_name(m.to.x - m.from.x, m.to.y - m.from.y).to_string();
            if move_ok[&m.robot] {
                plan.moves.insert(m.robot, m.to);
                record(&mut plan, m.robot, "move", arg, codes::OK);
            } else {
                record(&mut plan, m.robot, "move", arg, codes::CHAIN_BLOCKED);
            }
        }
        // drop 依赖：落点被机器人占用时，须其成功离开；否则 CELL_OCCUPIED。
        for d in &drop_winners {
            let ok = match self.robot_at(d.cell) {
                None => true,
                Some(occ) => move_ok.get(&occ).copied().unwrap_or(false),
            };
            if ok {
                res_acts.push(Res::Drop {
                    robot: d.robot,
                    cell: d.cell,
                    box_id: d.box_id,
                });
            } else {
                record(
                    &mut plan,
                    d.robot,
                    "drop",
                    format!("{},{}", d.cell.x, d.cell.y),
                    codes::CELL_OCCUPIED,
                );
            }
        }

        // ③ + ④ 剩余动作按机器人 id 升序原子占用资源。草稿状态 = 结算前
        // 世界 + 本 tick 已裁决转移；失败动作无部分效果；同一箱每 tick 至多
        // 转移一次，不允许在不同动作间接力。
        res_acts.sort_by_key(|a| a.robot());
        let mut box_holder: BTreeMap<Id, Option<Id>> = self
            .ground_boxes
            .iter()
            .map(|(id, b)| (*id, b.holder))
            .collect();
        let mut carry: BTreeMap<Id, Option<Id>> =
            self.robots.iter().map(|(id, r)| (*id, r.carry)).collect();
        let mut shelf_fill: BTreeMap<Id, usize> = self
            .shelves
            .iter()
            .map(|(id, s)| (*id, s.box_ids.len()))
            .collect();
        let mut vehicle_fill: BTreeMap<Id, usize> = self
            .vehicles
            .iter()
            .map(|(id, v)| (*id, v.box_ids.len()))
            .collect();
        let mut transferred: BTreeSet<Id> = BTreeSet::new();
        let mut charger_busy: BTreeSet<Id> = BTreeSet::new();

        for act in res_acts {
            match act {
                Res::Charge { robot, charger } => {
                    if !charger_busy.insert(charger) {
                        // 一桩每 tick 至多一台；同 tick 竞争由机器人 id 裁决。
                        record(
                            &mut plan,
                            robot,
                            "charge",
                            charger.to_string(),
                            codes::CHARGER_BUSY,
                        );
                        continue;
                    }
                    let r = &self.robots[&robot];
                    let gain = CHARGE_PER_TICK.min(r.energy_max.saturating_sub(r.energy));
                    *plan.energy.entry(robot).or_insert(0) += gain as i32;
                    record(&mut plan, robot, "charge", charger.to_string(), codes::OK);
                }
                Res::Take {
                    robot,
                    target,
                    box_id,
                } => {
                    // 转交目标机器人本 tick 成功移动 → 失败（先判移动后判资源）。
                    if let TargetRef::Robot(t) = target
                        && move_ok.get(&t).copied().unwrap_or(false)
                    {
                        record(
                            &mut plan,
                            robot,
                            "take",
                            target.id().to_string(),
                            codes::TARGET_MOVED,
                        );
                        continue;
                    }
                    let contested = transferred.contains(&box_id)
                        || box_holder.get(&box_id).copied().flatten() != Some(target.id())
                        || carry[&robot].is_some(); // 被动接收已占单箱容量
                    if contested {
                        record(
                            &mut plan,
                            robot,
                            "take",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(robot));
                    carry.insert(robot, Some(box_id));
                    transferred.insert(box_id);
                    // 取出释放容器装载（同 tick 后续 give 可用该空位，按 id 序裁决）。
                    match target {
                        TargetRef::Shelf(shelf_id) => {
                            let f = shelf_fill.get_mut(&shelf_id).expect("已索引");
                            *f = f.saturating_sub(1);
                        }
                        TargetRef::Vehicle(vehicle_id) => {
                            let f = vehicle_fill.get_mut(&vehicle_id).expect("已索引");
                            *f = f.saturating_sub(1);
                        }
                        TargetRef::Robot(_) => {}
                    }
                    let pos = self.robots[&robot].pos;
                    plan.box_moves.push((box_id, Some(robot), pos));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_TAKE as i32;
                    record(&mut plan, robot, "take", target.id().to_string(), codes::OK);
                }
                Res::Give {
                    robot,
                    target,
                    box_id,
                } => {
                    if let TargetRef::Robot(t) = target
                        && move_ok.get(&t).copied().unwrap_or(false)
                    {
                        record(
                            &mut plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_MOVED,
                        );
                        continue;
                    }
                    if transferred.contains(&box_id) || carry[&robot] != Some(box_id) {
                        record(
                            &mut plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    let placed = match target {
                        TargetRef::Shelf(shelf_id) => {
                            if shelf_fill[&shelf_id] < self.shelves[&shelf_id].capacity {
                                *shelf_fill.get_mut(&shelf_id).expect("已索引") += 1;
                                plan.box_moves.push((
                                    box_id,
                                    Some(shelf_id),
                                    self.shelves[&shelf_id].pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                        TargetRef::Vehicle(vehicle_id) => {
                            if vehicle_fill[&vehicle_id] < self.vehicle_capacity(vehicle_id) {
                                *vehicle_fill.get_mut(&vehicle_id).expect("已索引") += 1;
                                plan.box_moves.push((
                                    box_id,
                                    Some(vehicle_id),
                                    self.vehicles[&vehicle_id].interact_pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                        TargetRef::Robot(target_robot) => {
                            if carry[&target_robot].is_none() {
                                carry.insert(target_robot, Some(box_id));
                                plan.box_moves.push((
                                    box_id,
                                    Some(target_robot),
                                    self.robots[&target_robot].pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                    };
                    if !placed {
                        record(
                            &mut plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(target.id()));
                    carry.insert(robot, None);
                    transferred.insert(box_id);
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_GIVE as i32;
                    record(&mut plan, robot, "give", target.id().to_string(), codes::OK);
                }
                Res::Pick {
                    robot,
                    cell,
                    box_id,
                } => {
                    let contested = transferred.contains(&box_id)
                        || box_holder.get(&box_id).copied().flatten().is_some()
                        || carry[&robot].is_some();
                    if contested {
                        record(
                            &mut plan,
                            robot,
                            "pick",
                            format!("{},{}", cell.x, cell.y),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(robot));
                    carry.insert(robot, Some(box_id));
                    transferred.insert(box_id);
                    let pos = self.robots[&robot].pos;
                    plan.box_moves.push((box_id, Some(robot), pos));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_PICK as i32;
                    record(
                        &mut plan,
                        robot,
                        "pick",
                        format!("{},{}", cell.x, cell.y),
                        codes::OK,
                    );
                }
                Res::Drop {
                    robot,
                    cell,
                    box_id,
                } => {
                    if transferred.contains(&box_id) || carry[&robot] != Some(box_id) {
                        record(
                            &mut plan,
                            robot,
                            "drop",
                            format!("{},{}", cell.x, cell.y),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, None);
                    carry.insert(robot, None);
                    transferred.insert(box_id);
                    plan.box_moves.push((box_id, None, cell));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_DROP as i32;
                    record(
                        &mut plan,
                        robot,
                        "drop",
                        format!("{},{}", cell.x, cell.y),
                        codes::OK,
                    );
                }
            }
        }

        // ⑤ 车辆离场（按草稿装载量判定；车辆 id 升序）：入库车卸空、出库车
        // 装满即离场。离场 → 订单完成与收款 → 装卸位释放的次序在 commit 固化。
        for (vid, v) in &self.vehicles {
            let Some(order) = self.my_orders.get(&v.order_id) else {
                continue;
            };
            let fill = vehicle_fill.get(vid).copied().unwrap_or(v.box_ids.len());
            let depart = match v.kind {
                VehicleKind::In => fill == 0,
                VehicleKind::Out => fill as u32 >= order.qty,
            };
            if !depart {
                continue;
            }
            let revenue_milli = match order.side {
                // 买单（玩家卖出）装满离场时收款；卖单款项已付。
                OrderSide::Buy => (order.qty as i64).saturating_mul(order.unit_price_milli),
                OrderSide::Sell => 0,
            };
            plan.departures.push(Departure {
                vehicle_id: *vid,
                order_id: v.order_id,
                port_id: order.port.expect("在场车辆必有关联装卸口"),
                revenue_milli,
            });
        }
        plan
    }

    /// 第 3 段：原子提交。全部效果一次性应用，次序：移动 → 电量 → 货物
    /// 转移 → 车辆离场（离场 → 订单完成与收款 → 装卸位释放）。
    fn commit(&mut self, mut plan: SettlementPlan) -> BTreeMap<Id, LastResult> {
        for (robot_id, to) in &plan.moves {
            if let Some(r) = self.robots.get_mut(robot_id) {
                r.pos = *to;
            }
        }
        for (robot_id, delta) in &plan.energy {
            if let Some(r) = self.robots.get_mut(robot_id) {
                let e = r.energy as i64 + *delta as i64;
                r.energy = e.clamp(0, r.energy_max as i64) as u32;
            }
        }
        for (box_id, new_holder, new_pos) in &plan.box_moves {
            let Some(b) = self.ground_boxes.get_mut(box_id) else {
                continue;
            };
            // 清旧容器引用（id 跨类型唯一，命中恰一处）。
            if let Some(old) = b.holder {
                if let Some(r) = self.robots.get_mut(&old) {
                    if r.carry == Some(*box_id) {
                        r.carry = None;
                    }
                } else if let Some(s) = self.shelves.get_mut(&old) {
                    s.box_ids.retain(|x| x != box_id);
                } else if let Some(v) = self.vehicles.get_mut(&old) {
                    v.box_ids.retain(|x| x != box_id);
                }
            }
            // 立新容器引用。
            if let Some(h) = new_holder {
                if let Some(r) = self.robots.get_mut(h) {
                    r.carry = Some(*box_id);
                } else if let Some(s) = self.shelves.get_mut(h) {
                    s.box_ids.push(*box_id);
                } else if let Some(v) = self.vehicles.get_mut(h) {
                    v.box_ids.push(*box_id);
                }
            }
            b.holder = *new_holder;
            b.pos = *new_pos;
        }
        for dep in std::mem::take(&mut plan.departures) {
            // 车辆离场：连同车上货物退出世界。
            if let Some(v) = self.vehicles.remove(&dep.vehicle_id) {
                for b in &v.box_ids {
                    self.ground_boxes.remove(b);
                }
                // 订单完成与收款。
                self.my_orders.remove(&dep.order_id);
                self.gold_milli = self.gold_milli.saturating_add(dep.revenue_milli);
                // 装卸位释放。
                if let Some(p) = self.ports.get_mut(&dep.port_id)
                    && p.docked_vehicle == Some(v.id)
                {
                    p.docked_vehicle = None;
                }
            }
        }
        self.intents.clear();
        self.last_results = plan.results.clone();
        plan.results
    }

    /// 结算后收尾（阶段 5 最小版）：tick +1。计息 / 渲染快照不在 B1。
    pub fn end_tick(&mut self) {
        self.tick += 1;
    }

    /// 确定性回归用的世界摘要（排列测试对比）。覆盖全部影响后续行为的状态。
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |v: u64, h: &mut u64| {
            *h ^= v;
            *h = h.wrapping_mul(0x100000001b3);
        };
        let mix_bytes = |s: &str, h: &mut u64| {
            for b in s.as_bytes() {
                mix(*b as u64, h);
            }
        };
        mix(self.tick, &mut h);
        mix(self.gold_milli as u64, &mut h);
        mix(self.debt_milli as u64, &mut h);
        mix(self.next_id, &mut h);
        mix(self.seed, &mut h);
        for w in self.rng_port.state_words() {
            mix(w, &mut h);
        }
        for p in &self.walls {
            mix(p.x as u64, &mut h);
            mix(p.y as u64, &mut h);
        }
        for (id, r) in &self.robots {
            mix(*id, &mut h);
            mix(r.pos.x as u64, &mut h);
            mix(r.pos.y as u64, &mut h);
            mix(r.energy as u64, &mut h);
            mix(r.energy_max as u64, &mut h);
            mix(r.carry.unwrap_or(0), &mut h);
        }
        for (id, res) in &self.last_results {
            mix(*id, &mut h);
            mix_bytes(&res.code, &mut h);
            mix_bytes(res.action, &mut h);
            mix_bytes(&res.arg, &mut h);
        }
        for (id, s) in &self.shelves {
            mix(*id, &mut h);
            mix(s.pos.x as u64, &mut h);
            mix(s.pos.y as u64, &mut h);
            mix(s.capacity as u64, &mut h);
            for b in &s.box_ids {
                mix(*b, &mut h);
            }
        }
        for (id, c) in &self.chargers {
            mix(*id, &mut h);
            mix(c.pos.x as u64, &mut h);
            mix(c.pos.y as u64, &mut h);
        }
        for (id, p) in &self.ports {
            mix(*id, &mut h);
            mix(p.pos.x as u64, &mut h);
            mix(p.pos.y as u64, &mut h);
            mix(p.docked_vehicle.unwrap_or(0), &mut h);
            mix(p.reserved_for.unwrap_or(0), &mut h);
        }
        for (id, v) in &self.vehicles {
            mix(*id, &mut h);
            mix(if v.kind.as_str() == "in" { 1 } else { 0 }, &mut h);
            mix(v.interact_pos.x as u64, &mut h);
            mix(v.interact_pos.y as u64, &mut h);
            mix(v.order_id, &mut h);
            mix_bytes(&v.goods_type, &mut h);
            for b in &v.box_ids {
                mix(*b, &mut h);
            }
        }
        for (id, b) in &self.ground_boxes {
            mix(*id, &mut h);
            mix(b.pos.x as u64, &mut h);
            mix(b.pos.y as u64, &mut h);
            mix(b.holder.unwrap_or(0), &mut h);
            mix_bytes(&b.goods_type, &mut h);
        }
        let mix_order = |o: &Order, h: &mut u64| {
            mix(o.id, h);
            mix(if o.side.as_str() == "buy" { 1 } else { 0 }, h);
            mix(o.qty as u64, h);
            mix(o.unit_price_milli as u64, h);
            mix(o.vehicle.unwrap_or(0), h);
            mix(o.port.unwrap_or(0), h);
            mix_bytes(&o.goods_type, h);
        };
        for o in self.listings.values() {
            mix_order(o, &mut h);
        }
        for o in self.my_orders.values() {
            mix_order(o, &mut h);
        }
        for a in &self.arrivals {
            mix(a.order_id, &mut h);
            mix(a.arrive_tick, &mut h);
        }
        h
    }
}

/// 结算计划：只读求解的产物、原子提交的输入（docs/architecture/02）。
#[derive(Debug, Clone, Default)]
pub struct SettlementPlan {
    pub results: BTreeMap<Id, LastResult>,
    /// 成功移动：机器人 → 目标格。
    pub moves: BTreeMap<Id, Position>,
    /// 电量变化（成功充电为正、成功动作耗电为负）。
    pub energy: BTreeMap<Id, i32>,
    /// 货物转移：(箱, 新容器, 新位置)；每箱每 tick 至多一条。
    pub box_moves: Vec<(Id, Option<Id>, Position)>,
    /// 车辆离场（含订单完成与收款、装卸位释放所需信息）。
    pub departures: Vec<Departure>,
}

/// 一辆车的离场：离场 → 订单完成与收款 → 装卸位释放（次序固化）。
#[derive(Debug, Clone)]
pub struct Departure {
    pub vehicle_id: Id,
    pub order_id: Id,
    pub port_id: Id,
    /// 买单（玩家卖出）装满离场的收款；卖单离场无收款。
    pub revenue_milli: MilliGold,
}

/// take 的镜像同步增量素材（api 层据此构造宿主回放增量）。
#[derive(Debug, Clone)]
pub struct TakeEffect {
    pub order_id: Id,
    pub port_id: Id,
    pub order: Order,
}

/// cancel 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct CancelEffect {
    pub order_id: Id,
    pub vehicle_id: Option<Id>,
    pub port_id: Option<Id>,
    /// 卖单（买入）取消的退款；买单为 0。
    pub refund_milli: MilliGold,
    pub fee_milli: MilliGold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyedKind {
    Robot,
    Shelf,
    Charger,
    Port,
    GroundBox,
}

/// destroy 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct DestroyEffect {
    pub kind: DestroyedKind,
    pub target_id: Id,
    pub refund_milli: MilliGold,
    /// 释放的静态障碍格（镜像 blocked 增量用）；被携带 / 在架货物无地面格。
    pub freed_cell: Option<Position>,
}

fn record(
    plan: &mut SettlementPlan,
    robot: Id,
    action: &'static str,
    arg: String,
    code: &'static str,
) {
    plan.results.insert(
        robot,
        LastResult {
            action,
            arg,
            code: code.to_string(),
        },
    );
}

fn intent_label(i: &Intent) -> (&'static str, String) {
    match i {
        Intent::Move { dx, dy, .. } => ("move", dir_name(*dx, *dy).to_string()),
        Intent::Charge { charger_id, .. } => ("charge", charger_id.to_string()),
        Intent::Take { target, .. } => ("take", target.id().to_string()),
        Intent::Give { target, .. } => ("give", target.id().to_string()),
        Intent::Pick { x, y, .. } => ("pick", format!("{x},{y}")),
        Intent::Drop { x, y, .. } => ("drop", format!("{x},{y}")),
    }
}

fn dir_name(dx: i32, dy: i32) -> &'static str {
    match (dx, dy) {
        (0, -1) => "NORTH",
        (0, 1) => "SOUTH",
        (-1, 0) => "WEST",
        (1, 0) => "EAST",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_world() -> World {
        let mut w = World::new_empty(10, 10, 100_000);
        w.add_robot(Position::new(1, 1));
        w
    }

    #[test]
    fn accept_static_checks() {
        let mut w = demo_world(); // 机器人 1 在 (1,1)
        let edge = w.add_robot(Position::new(0, 0));
        assert_eq!(w.accept_move(1, 2, 0), codes::INVALID_ARGUMENT);
        assert_eq!(w.accept_move(99, 1, 0), codes::NO_SUCH_OBJECT);
        assert_eq!(w.accept_move(edge, 0, -1), codes::OUT_OF_BOUNDS);
        assert_eq!(w.accept_move(edge, -1, 0), codes::OUT_OF_BOUNDS);
        w.add_wall(Position::new(2, 1));
        assert_eq!(w.accept_move(1, 1, 0), codes::CELL_BLOCKED);
        // 受理失败不消耗行动机会：EAST 被墙挡后，改提 SOUTH 可受理通过。
        assert_eq!(w.accept_move(1, 0, 1), codes::OK);
        // 受理通过即占用本 tick 行动机会，重复提交不静默覆盖。
        assert_eq!(w.accept_move(1, 0, 1), codes::ALREADY_ACTED);
        assert_eq!(w.accept_move(1, 1, 0), codes::ALREADY_ACTED);
    }

    #[test]
    fn simple_move_settles() {
        let mut w = demo_world();
        assert_eq!(w.accept_move(1, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&1].code, codes::OK);
        assert_eq!(w.robots[&1].pos, Position::new(2, 1));
        w.end_tick();
        assert_eq!(w.tick, 1);
        // 无受理动作 → last_result 为 None 语义（该机器人无记录）。
        let res = w.settle();
        assert!(!res.contains_key(&1));
    }

    #[test]
    fn contested_cell_min_id_wins_no_promotion() {
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1)); // id 小
        let b = w.add_robot(Position::new(3, 1));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::OK);
        assert_eq!(res[&b].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
    }

    #[test]
    fn swap_fails_ring_succeeds() {
        // 二元交换：不允许。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 1));

        // 网格上最小的“圈”是四格方形轮转（≥3 个机器人允许）。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        let c = w.add_robot(Position::new(2, 2));
        let d = w.add_robot(Position::new(1, 2));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, 0, 1), codes::OK);
        assert_eq!(w.accept_move(c, -1, 0), codes::OK);
        assert_eq!(w.accept_move(d, 0, -1), codes::OK);
        let res = w.settle();
        for id in [a, b, c, d] {
            assert_eq!(res[&id].code, codes::OK, "robot {id}");
        }
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 2));
        assert_eq!(w.robots[&c].pos, Position::new(1, 2));
        assert_eq!(w.robots[&d].pos, Position::new(1, 1));
    }

    #[test]
    fn chain_blocked_propagates() {
        // c 静止占格，b 试图进 c 的格，a 跟在 b 后面：b、a 均 CHAIN_BLOCKED。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        let c = w.add_robot(Position::new(3, 1));
        assert_eq!(w.accept_move(b, 1, 0), codes::OK);
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        let _ = c;
    }

    #[test]
    fn follow_into_vacated_cell() {
        // 前方机器人成功离开 → 后续允许跟进（目标机器人本 tick 成功离开）。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        assert_eq!(w.accept_move(b, 1, 0), codes::OK); // b → (3,1)
        assert_eq!(w.accept_move(a, 1, 0), codes::OK); // a → (2,1)（b 离开）
        let res = w.settle();
        assert_eq!(res[&b].code, codes::OK);
        assert_eq!(res[&a].code, codes::OK);
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
    }

    #[test]
    fn take_order_flow() {
        let mut w = World::new_empty(10, 10, 1_000_000);
        let port = w.add_port(Position::new(0, 5));
        let o1 = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
        let o2 = w.add_listing(OrderSide::Buy, "chip", 1, 7000);
        let (code, eff) = w.manage_take(o1);
        assert_eq!(code, codes::OK);
        let eff = eff.unwrap();
        assert_eq!(eff.port_id, port);
        assert_eq!(w.gold_milli, 1_000_000 - 2 * 5000);
        assert!(w.listings.contains_key(&o2) && !w.listings.contains_key(&o1));
        assert_eq!(w.my_orders[&o1].port, Some(port));
        // 车辆下一 tick 边界到场。
        w.end_tick();
        w.boundary_events();
        assert_eq!(w.vehicles.len(), 1);
        let v = w.vehicles.values().next().unwrap();
        assert_eq!(v.kind, VehicleKind::In);
        assert_eq!(v.box_ids.len(), 2);
        assert_eq!(w.ports[&port].docked_vehicle, Some(v.id));
        assert_eq!(w.my_orders[&o1].vehicle, Some(v.id));
    }

    #[test]
    fn take_rejects() {
        let mut w = World::new_empty(10, 10, 100);
        w.add_port(Position::new(0, 5));
        let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
        assert_eq!(w.manage_take(o).0, codes::NO_FUNDS);
        let mut w2 = World::new_empty(10, 10, 1_000_000);
        let o2 = w2.add_listing(OrderSide::Sell, "battery", 1, 100);
        assert_eq!(w2.manage_take(o2).0, codes::NO_FREE_PORT);
        assert_eq!(w.manage_take(999).0, codes::ORDER_GONE);
    }

    #[test]
    fn no_promotion_when_winner_dependency_fails() {
        // c 静止占格；a、b 争 c 的格：a（id 小）取得候选但因 c 不离开而
        // CHAIN_BLOCKED；b 保持 CELL_CONTESTED——候选失败不递补。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(3, 1));
        let c = w.add_robot(Position::new(2, 1)); // id 最大却占着目标格
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
        assert_eq!(w.robots[&c].pos, Position::new(2, 1));
    }

    /// 受理集合与结算结果与提交顺序无关：≤5 台全排列（docs/architecture/08
    /// 原型 B 回归的 move 子集；混合动作场景见 tests/b1_regression.rs）。
    #[test]
    fn settlement_permutation_invariant() {
        use ztw_model::EAST;
        // 布局：5 台机器人，两两争抢两个格、含一条链与一个静止占位者。
        let build = |w: &mut World| -> Vec<Id> {
            let mut ids = Vec::new();
            for (x, y) in [(1, 1), (3, 1), (5, 1), (2, 1), (4, 1)] {
                ids.push(w.add_robot(Position::new(x, y)));
            }
            ids
        };
        let dirs = [EAST, (0, 1), (-1, 0), EAST, (0, 1)];
        let accepts = |w: &mut World, ids: &Vec<Id>| -> Vec<&'static str> {
            ids.iter()
                .zip(dirs)
                .map(|(id, d)| w.accept_move(*id, d.0, d.1))
                .collect()
        };
        let baseline = {
            let mut w = World::new_empty(10, 10, 0);
            let ids = build(&mut w);
            let codes0 = accepts(&mut w, &ids);
            w.settle();
            (w.state_hash(), w.robots.clone(), codes0)
        };
        let mut perm = [0usize, 1, 2, 3, 4];
        loop {
            let mut w = World::new_empty(10, 10, 0);
            let ids = build(&mut w);
            let mut codes_p = vec![ztw_model::codes::OK; 5];
            for &i in &perm {
                let d = dirs[i];
                codes_p[i] = w.accept_move(ids[i], d.0, d.1);
            }
            w.settle();
            assert_eq!(w.state_hash(), baseline.0, "perm {perm:?}");
            assert_eq!(w.robots, baseline.1);
            assert_eq!(codes_p, baseline.2, "受理集合也必须一致：perm {perm:?}");
            if !next_perm(&mut perm) {
                break;
            }
        }
    }

    fn next_perm(p: &mut [usize; 5]) -> bool {
        let n = p.len();
        let mut i = n - 1;
        while i > 0 && p[i - 1] >= p[i] {
            i -= 1;
        }
        if i == 0 {
            return false;
        }
        let mut j = n - 1;
        while p[j] <= p[i - 1] {
            j -= 1;
        }
        p.swap(i - 1, j);
        p[i..].reverse();
        true
    }

    // ------------------------------------------------------------------
    // B1 核心语义冒烟（完整回归见 tests/b1_*.rs）
    // ------------------------------------------------------------------

    #[test]
    fn charge_flow_and_contention() {
        let mut w = demo_world();
        let c = w.add_charger(Position::new(1, 2)); // 与 (1,1) 相邻
        let other = w.add_robot(Position::new(1, 3)); // 也相邻 (1,2)
        w.robots.get_mut(&1).unwrap().energy = 40;
        assert_eq!(w.accept_charge(1), codes::OK);
        assert_eq!(w.accept_charge(other), codes::OK);
        let res = w.settle();
        assert_eq!(res[&1].code, codes::OK);
        assert_eq!(res[&other].code, codes::CHARGER_BUSY);
        assert_eq!(w.robots[&1].energy, 40 + CHARGE_PER_TICK);
        assert_eq!(w.robots[&other].energy, 100);
        // 每 tick 重新申请：下一 tick 另一台可充。
        w.end_tick();
        assert_eq!(w.accept_charge(other), codes::OK);
        let res = w.settle();
        assert_eq!(res[&other].code, codes::OK);
        let _ = c;
    }

    #[test]
    fn pick_drop_and_energy_costs() {
        let mut w = demo_world();
        let b = w.add_ground_box("water", Position::new(2, 1));
        assert_eq!(w.accept_pick(1, 2, 1), codes::OK);
        let res = w.settle();
        assert_eq!(res[&1].code, codes::OK);
        assert_eq!(w.robots[&1].carry, Some(b));
        assert_eq!(w.robots[&1].energy, 100 - ENERGY_COST_PICK);
        // 放下是下一个 tick 的动作（每 tick 至多一个动作）。
        w.end_tick();
        assert_eq!(w.accept_drop(1, 2, 1, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&1].code, codes::OK);
        assert_eq!(w.robots[&1].carry, None);
        assert_eq!(w.ground_boxes[&b].pos, Position::new(2, 1));
        assert_eq!(w.ground_boxes[&b].holder, None);
        assert_eq!(
            w.robots[&1].energy,
            100 - ENERGY_COST_PICK - ENERGY_COST_DROP
        );
    }

    #[test]
    fn in_vehicle_emptying_departs() {
        let mut w = World::new_empty(10, 10, 1_000_000);
        let port = w.add_port(Position::new(0, 5));
        let o = w.add_listing(OrderSide::Sell, "battery", 1, 5000);
        w.manage_take(o);
        w.end_tick();
        w.boundary_events();
        let vid = w.ports[&port].docked_vehicle.expect("车辆已到场");
        let v = &w.vehicles[&vid];
        let box_id = v.box_ids[0];
        // 车停在 (0,5)，机器人移到相邻 (1,5) 后卸货到地面即可清空。
        let r = w.add_robot(Position::new(1, 5));
        assert_eq!(w.accept_take(r, vid, box_id), codes::OK);
        let res = w.settle();
        assert_eq!(res[&r].code, codes::OK);
        // 同一结算内：车辆离场、订单完成、装卸位释放。
        assert!(!w.vehicles.contains_key(&vid));
        assert!(!w.my_orders.contains_key(&o));
        assert_eq!(w.ports[&port].docked_vehicle, None);
        assert_eq!(w.robots[&r].carry, Some(box_id));
        assert_eq!(w.gold_milli, 1_000_000 - 5000); // 卖单无离场收款
    }

    #[test]
    fn out_vehicle_filling_departs_and_pays() {
        let mut w = World::new_empty(10, 10, 1_000_000);
        let port = w.add_port(Position::new(0, 5));
        let o = w.add_listing(OrderSide::Buy, "chip", 1, 7000);
        w.manage_take(o);
        w.end_tick();
        w.boundary_events();
        let vid = w.ports[&port].docked_vehicle.unwrap();
        let r = w.add_robot(Position::new(1, 5));
        let b = w.add_ground_box("chip", Position::new(2, 5));
        assert_eq!(w.accept_pick(r, 2, 5), codes::OK);
        let res = w.settle();
        assert_eq!(res[&r].code, codes::OK);
        w.end_tick();
        assert_eq!(w.accept_give(r, vid, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&r].code, codes::OK);
        assert!(!w.vehicles.contains_key(&vid));
        assert!(!w.my_orders.contains_key(&o));
        assert_eq!(w.gold_milli, 1_000_000 + 7000); // 买单不预付、装满收款
        assert!(!w.ground_boxes.contains_key(&b)); // 货随车离场
        assert_eq!(w.robots[&r].carry, None);
    }

    #[test]
    fn cancel_restores_and_charges_fee() {
        let mut w = World::new_empty(10, 10, 1_000_000);
        let port = w.add_port(Position::new(0, 5));
        let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
        w.manage_take(o);
        let paid = w.gold_milli;
        // 车辆未到场（初态）即可取消，退回购价扣 10% 手续费。
        let (code, eff) = w.manage_cancel(o);
        assert_eq!(code, codes::OK);
        let eff = eff.unwrap();
        assert_eq!(eff.fee_milli, 2 * 5000 / 10);
        assert_eq!(w.gold_milli, paid + 2 * 5000 - 2 * 5000 / 10);
        assert!(!w.my_orders.contains_key(&o));
        assert_eq!(w.ports[&port].reserved_for, None);
        w.end_tick();
        w.boundary_events();
        assert!(w.vehicles.is_empty()); // arrival 已撤销
    }

    #[test]
    fn destroy_rejects_and_refunds() {
        let mut w = demo_world();
        let s = w.add_shelf(Position::new(4, 4));
        let gold = w.gold_milli;
        assert_eq!(w.manage_destroy(s).0, codes::OK);
        assert_eq!(w.gold_milli, gold + PRICE_SHELF / 2);
        assert_eq!(w.manage_destroy(999).0, codes::NO_SUCH_OBJECT);
        assert_eq!(w.manage_destroy(1).0, codes::OK); // 空载机器人可毁
    }
}
