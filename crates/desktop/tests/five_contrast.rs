//! B2 五机对比验收（docs/architecture/08 原型 B）：naive 贪心 vs 分流。
//! 脚本本体在 web/apps/game/src/scripts/demos/（编辑器示例同一份文本，
//! 单一事实源）。断言依据见 records/b2-playthrough.md。

use std::time::Instant;

use ztw_api::harness::{DiagTapKind, Session, SessionConfig};
use ztw_desktop::{hostbin, scenario};

const NAIVE: &str = include_str!("../../../web/apps/game/src/scripts/demos/demo_five_naive.js");
const LANES: &str = include_str!("../../../web/apps/game/src/scripts/demos/demo_five_lanes.js");

struct Report {
    /// 六单全部了结的 tick（None = 上限内未完成）。
    done_tick: Option<u64>,
    orders_done: usize,
    accept_fail: usize,
    contested: u64,
    chain_blocked: u64,
    cell_occupied: u64,
    gold_milli: i64,
}

fn run_five(code: &str, max_ticks: u64) -> Report {
    let bin = hostbin::resolve_host_bin().expect("宿主二进制应可解析");
    let mut s = Session::new(SessionConfig::new(bin), scenario::build(&scenario::B2_FIVE));
    let init = s.load_code(code);
    assert!(init.ok, "脚本加载应成功：{:?}", init.fault);

    let mut settle_hist: std::collections::BTreeMap<String, u64> = Default::default();
    let mut rep = Report {
        done_tick: None,
        orders_done: 0,
        accept_fail: 0,
        contested: 0,
        chain_blocked: 0,
        cell_occupied: 0,
        gold_milli: 0,
    };
    let mut fails: std::collections::BTreeMap<String, u64> = Default::default();
    for _ in 0..max_ticks {
        for d in s.take_diag() {
            match d.kind {
                DiagTapKind::AcceptFail => {
                    rep.accept_fail += 1;
                    *fails.entry(d.op.clone() + ":" + &d.code).or_insert(0) += 1;
                }
                DiagTapKind::OrderDone => rep.orders_done += 1,
                DiagTapKind::Manage => {}
            }
        }
        let out = s.tick();
        assert_eq!(
            out.kind,
            ztw_api::harness::OutcomeKind::Ok,
            "脚本不应产生运行时故障"
        );
        for (_, code) in &out.settle {
            match code.as_str() {
                "CELL_CONTESTED" => rep.contested += 1,
                "CHAIN_BLOCKED" => rep.chain_blocked += 1,
                "CELL_OCCUPIED" => rep.cell_occupied += 1,
                _ => {}
            }
            *settle_hist.entry(code.clone()).or_insert(0) += 1;
        }
        if rep.orders_done >= 6 && rep.done_tick.is_none() {
            rep.done_tick = Some(out.tick);
        }
        if rep.done_tick.is_some() {
            break;
        }
    }
    // 排空最后的诊断（完成 tick 之后可能还有 OrderDone 事件待取）。
    for d in s.take_diag() {
        if d.kind == DiagTapKind::OrderDone {
            rep.orders_done += 1;
        }
    }
    rep.gold_milli = s.world.gold_milli;
    println!("受理失败直方图 {fails:?}");
    println!("结算码直方图 {settle_hist:?}");
    println!(
        "五机报告：done={:?} orders={} accept_fail={} contested={} chain={} occupied={} gold={}",
        rep.done_tick,
        rep.orders_done,
        rep.accept_fail,
        rep.contested,
        rep.chain_blocked,
        rep.cell_occupied,
        rep.gold_milli
    );
    rep
}

#[test]
fn five_contrast_naive_jams_lanes_clears() {
    let t0 = Instant::now();
    let naive = run_five(NAIVE, 400);
    let lanes = run_five(LANES, 400);
    println!("两场景合计耗时 {:?}", t0.elapsed());

    // lanes：六单全部了结，且争抢码显著少于 naive。
    assert_eq!(lanes.orders_done, 6, "分流版应完成全部六单");
    let lanes_done = lanes.done_tick.expect("分流版应在时限内完成");
    assert!(
        lanes.contested + lanes.chain_blocked <= 10,
        "分流版争抢应罕见"
    );

    // naive：同预算内要么未完成、要么以大量争抢码为代价完成。
    let naive_jammed = naive.contested + naive.chain_blocked + naive.cell_occupied;
    let lanes_jammed = lanes.contested + lanes.chain_blocked + lanes.cell_occupied;
    assert!(
        naive.done_tick.is_none() || naive.done_tick.unwrap() > lanes_done,
        "贪心版若完成也必须显著慢于分流版：naive={:?} lanes={lanes_done}",
        naive.done_tick
    );
    assert!(
        naive_jammed > lanes_jammed,
        "贪心版争抢码应显著更多：naive={naive_jammed} lanes={lanes_jammed}"
    );
}
