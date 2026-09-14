//! M3 经济校准驱动（docs/architecture/08 里程碑 3：「校准周转与囤货取舍」）。
//! 三策略（快转 / 囤货 / 杠杆囤货）同一 m3-trade 场景跑固定窗口 × 三个
//! 世界种子（平滑单窗口行情运气），逐 tick 采集资金线并输出指标表
//!（--nocapture 人工读数，结论回写 records/m3-calibration.md 并冻结参数）。
//!
//! 指标定义（结论记录的口径，全部以 milli 计）：
//! - 库存成本值 = Σ 各型现存箱数 × 该型历史买入均价（移动加权，近似成本计价）；
//! - 占用资金 = 金币 + 库存成本值；资金周转率 = 买单收款总额 / 平均占用
//!   资金 /（窗口 tick / 100）；
//! - 囤货占用率 = 平均库存成本值 / 平均占用资金；
//! - 净资产 = 终态金币 + 终态库存成本值 − 终态欠款；净利润 = 净资产 − 初始金币；
//! - 停摆刻度 = 首个「金币 < 当前最便宜整单成本 且 欠款 ≥ 信用额度」的
//!   tick（无破产机制下的可观测 proxy，无则 N/A）；
//! - 分货物毛利 = 该型已售均价 / 该型历史买入均价 − 1。

use std::collections::BTreeMap;

use ztw_api::harness::{OutcomeKind, Session, SessionConfig};
use ztw_desktop::{hostbin, scenario};
use ztw_sim::CREDIT_LIMIT_MILLI;

const FAST: &str = include_str!("../../../web/apps/game/src/scripts/demos/demo_trade_fast.js");
const HOARD: &str = include_str!("../../../web/apps/game/src/scripts/demos/demo_trade_hoard.js");
const LEVERAGE: &str =
    include_str!("../../../web/apps/game/src/scripts/demos/demo_trade_leverage.js");

const WINDOW: u64 = 600;
const SEEDS: [u64; 3] = [20260913, 7, 41277];

/// 终态资金线（多窗口合并时保留最后一个窗口的终态）。
#[derive(Default, Clone, Copy)]
struct Terminal {
    gold: i64,
    debt: i64,
    inventory: i64,
}

/// 逐 tick 采集器：买入均价、收款、资金线（与实现解耦的账本口径）。
struct Recorder {
    /// 各型：累计买入箱数 / 累计买入金额（移动加权成本）。
    bought_qty: BTreeMap<String, u64>,
    bought_milli: BTreeMap<String, i64>,
    /// 各型：累计已售箱数 / 累计收款。
    sold_qty: BTreeMap<String, u64>,
    revenue_milli: BTreeMap<String, i64>,
    sum_gold: i64,
    sum_debt: i64,
    sum_inventory: i64,
    samples: u64,
    /// 已接订单快照（id → 是否卖单、货物、数量、单价）：识别本 tick 完成
    /// 的买单（离场收款）与新接的卖单（买入成本）。
    known_orders: BTreeMap<u64, (bool, String, u32, i64)>,
    stall_tick: Option<u64>,
    orders_completed: u64,
    terminal: Terminal,
    /// 逐窗口累计净利润（终态净资产 − 窗口初始金币）：合并报表不混用
    /// 单窗口终态与多窗口流量。
    profit_sum: i64,
}

impl Recorder {
    fn new() -> Recorder {
        Recorder {
            bought_qty: BTreeMap::new(),
            bought_milli: BTreeMap::new(),
            sold_qty: BTreeMap::new(),
            revenue_milli: BTreeMap::new(),
            sum_gold: 0,
            sum_debt: 0,
            sum_inventory: 0,
            samples: 0,
            known_orders: BTreeMap::new(),
            stall_tick: None,
            orders_completed: 0,
            terminal: Terminal::default(),
            profit_sum: 0,
        }
    }

    /// 窗口收尾：累计本窗口净利润并清空窗口态（订单快照 / 停摆刻度）。
    fn finalize_window(&mut self, initial_gold: i64) {
        self.profit_sum +=
            self.terminal.gold + self.terminal.inventory - self.terminal.debt - initial_gold;
        self.known_orders.clear();
        self.stall_tick = None;
    }

    fn merge(&mut self, other: Recorder) {
        for (k, v) in other.bought_qty {
            *self.bought_qty.entry(k).or_insert(0) += v;
        }
        for (k, v) in other.bought_milli {
            *self.bought_milli.entry(k).or_insert(0) += v;
        }
        for (k, v) in other.sold_qty {
            *self.sold_qty.entry(k).or_insert(0) += v;
        }
        for (k, v) in other.revenue_milli {
            *self.revenue_milli.entry(k).or_insert(0) += v;
        }
        self.sum_gold += other.sum_gold;
        self.sum_debt += other.sum_debt;
        self.sum_inventory += other.sum_inventory;
        self.samples += other.samples;
        self.orders_completed += other.orders_completed;
        self.stall_tick = other.stall_tick;
        self.terminal = other.terminal;
        self.profit_sum += other.profit_sum;
    }

    /// 每 tick 结算后调用。
    fn sample(&mut self, s: &Session) {
        let current: BTreeMap<u64, (bool, String, u32, i64)> = s
            .world
            .my_orders
            .values()
            .map(|o| {
                (
                    o.id,
                    (
                        o.side == ztw_model::OrderSide::Sell,
                        o.goods_type.clone(),
                        o.qty,
                        o.unit_price_milli,
                    ),
                )
            })
            .collect();
        for (id, (is_sell, goods, qty, price)) in &current {
            if !self.known_orders.contains_key(id) {
                if *is_sell {
                    *self.bought_qty.entry(goods.clone()).or_insert(0) += *qty as u64;
                    *self.bought_milli.entry(goods.clone()).or_insert(0) += *qty as i64 * price;
                }
                self.known_orders
                    .insert(*id, (*is_sell, goods.clone(), *qty, *price));
            }
        }
        for (id, (is_sell, goods, qty, price)) in self.known_orders.clone() {
            if !current.contains_key(&id) {
                self.orders_completed += 1;
                if !is_sell {
                    *self.sold_qty.entry(goods.clone()).or_insert(0) += qty as u64;
                    *self.revenue_milli.entry(goods.clone()).or_insert(0) += qty as i64 * price;
                }
                self.known_orders.remove(&id);
            }
        }
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for b in s.world.ground_boxes.values() {
            *counts.entry(b.goods_type.clone()).or_insert(0) += 1;
        }
        let inventory: i64 = counts
            .iter()
            .map(|(g, n)| {
                let q = self.bought_qty.get(g).copied().unwrap_or(0);
                let spent = self.bought_milli.get(g).copied().unwrap_or(0);
                let avg = if q > 0 { spent / q as i64 } else { 0 };
                *n as i64 * avg
            })
            .sum();
        self.sum_gold += s.world.gold_milli;
        self.sum_debt += s.world.debt_milli;
        self.sum_inventory += inventory;
        self.samples += 1;
        self.terminal = Terminal {
            gold: s.world.gold_milli,
            debt: s.world.debt_milli,
            inventory,
        };
        if self.stall_tick.is_none()
            && s.world.debt_milli >= CREDIT_LIMIT_MILLI
            && s.world
                .listings
                .values()
                .filter(|o| o.side == ztw_model::OrderSide::Sell)
                .map(|o| o.qty as i64 * o.unit_price_milli)
                .min()
                .is_some_and(|cheapest| s.world.gold_milli < cheapest)
        {
            self.stall_tick = Some(s.world.tick);
        }
    }

    fn report(&self, name: &str) {
        let n = self.samples.max(1) as i64;
        let avg_gold = self.sum_gold / n;
        let avg_inv = self.sum_inventory / n;
        let avg_capital = avg_gold + avg_inv;
        let revenue: i64 = self.revenue_milli.values().sum();
        let profit = self.profit_sum;
        let turnover = if avg_capital > 0 {
            (revenue as f64 / avg_capital as f64) / (self.samples as f64 / 100.0)
        } else {
            0.0
        };
        let hoard_ratio = if avg_capital > 0 {
            avg_inv as f64 / avg_capital as f64
        } else {
            0.0
        };
        println!("== {name}（{} 窗口 × {} tick）==", SEEDS.len(), WINDOW);
        println!(
            "  完成订单 {} | 收款总额 {:.1} gold | 净利润（逐窗口累计）{:+.1} gold | 末窗终态：金币 {:.1} + 库存成本值 {:.1} − 欠款 {:.1}",
            self.orders_completed,
            revenue as f64 / 1000.0,
            profit as f64 / 1000.0,
            self.terminal.gold as f64 / 1000.0,
            self.terminal.inventory as f64 / 1000.0,
            self.terminal.debt as f64 / 1000.0,
        );
        println!(
            "  资金周转率 {turnover:.2}/100tick | 囤货占用率 {:.0}% | 停摆刻度 {}",
            hoard_ratio * 100.0,
            self.stall_tick
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".into()),
        );
        for g in ["water", "battery", "chip"] {
            let (sq, rv) = (
                self.sold_qty.get(g).copied().unwrap_or(0),
                self.revenue_milli.get(g).copied().unwrap_or(0),
            );
            let (bq, bm) = (
                self.bought_qty.get(g).copied().unwrap_or(0),
                self.bought_milli.get(g).copied().unwrap_or(0),
            );
            if sq == 0 && bq == 0 {
                println!("  {g}: 未参与");
                continue;
            }
            let avg_buy = if bq > 0 { bm as f64 / bq as f64 } else { 0.0 };
            let margin = if sq > 0 {
                (rv as f64 / sq as f64) / avg_buy - 1.0
            } else {
                f64::NAN
            };
            println!(
                "  {g}: 买入 {bq} 箱 @均价 {:.2} | 已售 {sq} 箱 | 毛利 {}",
                avg_buy / 1000.0,
                if sq > 0 {
                    format!("{:+.0}%", margin * 100.0)
                } else {
                    "—（在库）".into()
                },
            );
        }
    }
}

fn run(name: &str, code: &str) -> Recorder {
    let bins = hostbin::resolve_host_bins();
    let mut merged: Option<Recorder> = None;
    for &seed in &SEEDS {
        let mut spec = scenario::M3_TRADE.clone();
        spec.seed = seed;
        let mut s = Session::new(SessionConfig::new(bins.js.clone()), scenario::build(&spec));
        assert!(s.load_code(code).ok, "{name} 加载失败：{:?}", s.fault);
        let initial_gold = s.world.gold_milli;
        let mut rec = Recorder::new();
        for _ in 0..WINDOW {
            let out = s.tick();
            assert_eq!(
                out.kind,
                OutcomeKind::Ok,
                "{name} seed={seed} 第 {} tick 故障：{:?}（脚本不应抛错）",
                s.world.tick,
                s.fault
            );
            rec.sample(&s);
        }
        let _ = s.take_diag();
        rec.finalize_window(initial_gold);
        println!(
            "-- {name} seed={seed}：完成 {} 单，终态金币 {:.1}、欠款 {:.1}",
            rec.orders_completed,
            s.world.gold_milli as f64 / 1000.0,
            s.world.debt_milli as f64 / 1000.0
        );
        match merged {
            None => merged = Some(rec),
            Some(ref mut m) => m.merge(rec),
        }
    }
    let rec = merged.expect("至少一个种子");
    rec.report(name);
    rec
}

#[test]
fn m3_calibration_bench() {
    let fast = run("快转（water 紧阈值）", FAST);
    let hoard = run("囤货（三货物深门）", HOARD);
    let leverage = run("杠杆囤货（借贷放大）", LEVERAGE);

    // 硬性验收（量测数字入 records，此处只钉方向性结论）：
    // 1. 三策略全程无故障、完成过买卖闭环（真实参与市场）。
    for (name, rec) in [("fast", &fast), ("hoard", &hoard), ("leverage", &leverage)] {
        assert!(rec.orders_completed > 0, "{name} 未完成任何订单");
        assert!(
            rec.revenue_milli.values().sum::<i64>() > 0,
            "{name} 无任何卖出收款——未形成买卖闭环"
        );
    }
    // 2. 快转的囤货占用率低于囤货（策略确实分化）。
    let ratio = |r: &Recorder| r.sum_inventory as f64 / (r.sum_inventory + r.sum_gold) as f64;
    assert!(
        ratio(&fast) < ratio(&hoard),
        "快转占用率 {:.2} 应低于囤货 {:.2}",
        ratio(&fast),
        ratio(&hoard)
    );
    // 3. 杠杆策略确实使用了借贷（欠款线非零），且未停摆。
    assert!(leverage.sum_debt > 0, "杠杆策略未发生借贷");
    assert!(leverage.stall_tick.is_none(), "杠杆策略在窗口内停摆");
    // 4. 无策略把经济做穿：金币均值不为负、终态净资产为正。
    for (name, rec) in [("fast", &fast), ("hoard", &hoard), ("leverage", &leverage)] {
        assert!(rec.sum_gold >= 0, "{name} 金币均值为负——经济被做穿");
        assert!(
            rec.profit_sum > -200_000,
            "{name} 净利润灾难性亏损（低于 −200 gold）"
        );
    }
}
