//! C1 存档数据面验收：快照往返保真（state_hash 等价 + 轨迹续流等价，
//! docs/architecture/08 原型 C 存档条款）。模式沿用 B2/M3 先例：固定
//! 种子枚举 × 逐 tick 断言，不引入属性测试依赖。

mod common;

use ztw_model::{Id, OrderSide, Position, codes};
use ztw_sim::{World, Xoshiro256};

/// 推进 n 个完整 tick（无意图；市场与到场事件照常演化）。
fn advance(w: &mut World, n: u64) {
    for _ in 0..n {
        w.boundary_events();
        w.settle();
        w.end_tick();
    }
}

/// 内容丰富 世界：市场 + 各类实体 + 已接订单（待到场车辆）+ 欠款 +
/// 上一 tick 结算结果（move / charge 两种动作名入 last_results）。
fn rich_world(seed: u64) -> World {
    let mut w = World::new_empty(12, 12, 2_000_000)
        .with_seed(seed)
        .with_market();
    w.add_wall(Position::new(6, 6));
    let robot = w.add_robot(Position::new(1, 1));
    w.add_shelf(Position::new(2, 3));
    // 紧邻 move 目标格 (2,1)：第二个 tick 的 charge 可受理。
    w.add_charger(Position::new(2, 2));
    w.add_dock(Position::new(0, 4), (1, 0));
    w.add_ground_box("water", Position::new(3, 3));
    // 借款：欠款与复利状态入档。
    assert_eq!(w.manage_borrow(50_000).0, codes::OK);
    // 接一张卖单：my_orders + 预留装卸位 + 待到场车辆入档。
    let sell: Option<Id> = w
        .listings
        .values()
        .find(|o| o.side == OrderSide::Sell)
        .map(|o| o.id);
    let sell = sell.expect("市场初始板面必有卖单");
    assert_eq!(w.manage_take(sell).0, codes::OK);
    // 结算一个带意图的 tick：last_results 覆盖 move 与 charge 动作名。
    w.boundary_events();
    assert_eq!(w.accept_move(robot, 1, 0), codes::OK);
    w.settle();
    w.end_tick();
    w.boundary_events();
    assert_eq!(w.accept_charge(robot), codes::OK);
    w.settle();
    w.end_tick();
    w
}

/// 往返保真：多种子 × 多时机快照，恢复后 state_hash 与原世界一致、
/// 不变量仍成立（镜像漏字段即此处红——完备性基准见 state_hash）。
#[test]
fn snapshot_roundtrip_preserves_state_hash() {
    for seed in 0..8u64 {
        let mut w = rich_world(seed);
        for &at in &[0u64, 7, 60] {
            advance(&mut w, at);
            let restored = World::from_snapshot(w.to_snapshot()).expect("合法快照");
            assert_eq!(
                w.state_hash(),
                restored.state_hash(),
                "seed {seed} tick {} 往返后 state_hash 漂移",
                w.tick
            );
            common::check_invariants(&restored);
        }
    }
}

/// 恢复等价（docs/architecture/08 确定性第三套的存档版）：存档点之后
/// 逐 tick 轨迹与不间断运行全等——PRNG 续流、市场演化、到场事件、
/// 计息无一失真。
#[test]
fn snapshot_resume_matches_uninterrupted_trajectory() {
    let run = |mut w: World| {
        let mut v = Vec::new();
        for _ in 0..60 {
            w.boundary_events();
            w.settle();
            w.end_tick();
            v.push(w.state_hash());
        }
        v
    };
    for seed in [0u64, 7, 20260913] {
        let mut direct = rich_world(seed);
        advance(&mut direct, 40);
        let snap = direct.to_snapshot();
        let hashes = run(direct);
        let restored = World::from_snapshot(snap).expect("合法快照");
        assert_eq!(
            run(restored),
            hashes,
            "seed {seed} 读档后 60 tick 轨迹与不间断运行不一致"
        );
    }
}

/// PRNG 状态字往返：from_state_words(state_words()) 后续流一致；
/// 全零退化态防御与 derive 同款。
#[test]
fn rng_state_words_roundtrip() {
    for seed in [0u64, 1, 42, 20260913] {
        for name in ["dock", "market"] {
            let mut rng = Xoshiro256::derive(seed, name);
            rng.next_u64();
            rng.next_u64();
            let mut restored = Xoshiro256::from_state_words(rng.state_words());
            assert_eq!(
                rng.state_words(),
                restored.state_words(),
                "{name} 状态字往返"
            );
            assert_eq!(rng.next_u64(), restored.next_u64(), "{name} 续流一致");
        }
    }
    let zero = Xoshiro256::from_state_words([0; 4]);
    assert_ne!(zero.state_words(), [0; 4], "全零状态字须防御性偏移");
}

/// 动作名码表门禁：未知动作名（规则版本不匹配的旧档 / 坏档）明确报错。
#[test]
fn unknown_action_name_rejected() {
    let w = rich_world(3);
    let mut snap = w.to_snapshot();
    let key = *snap.last_results.keys().next().unwrap();
    snap.last_results.get_mut(&key).unwrap().action = "teleport".to_string();
    let err = World::from_snapshot(snap).expect_err("未知动作名须拒绝");
    assert!(err.contains("teleport"), "错误须点名未知动作名：{err}");
}
