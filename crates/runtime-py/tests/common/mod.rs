//! A1 Python 宿主集成测试公共辅助。

use ztw_api::harness::{Session, SessionConfig};
use ztw_model::Position;
use ztw_sim::World;

pub fn host_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ztw-host-py")
}

/// JS 宿主二进制：与 ztw-host-py 同目录（同一 target profile）。本包不依赖
/// ztw-runtime（bin 不随依赖构建），按兄弟路径解析；`cargo test --workspace`
/// （CI / just gate）恒可用，单独跑本包前先 `cargo build -p ztw-runtime`。
#[allow(dead_code)]
pub fn js_bin() -> std::path::PathBuf {
    let sibling = std::path::Path::new(env!("CARGO_BIN_EXE_ztw-host-py"))
        .parent()
        .expect("可执行目录")
        .join(format!("ztw-host-js{}", std::env::consts::EXE_SUFFIX));
    assert!(
        sibling.exists(),
        "未找到 {}——先 cargo build -p ztw-runtime（或 cargo test --workspace）",
        sibling.display()
    );
    sibling
}

#[allow(dead_code)]
pub fn demo_world() -> World {
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_shelf(Position::new(6, 3));
    w.add_charger(Position::new(2, 5));
    w.add_dock(Position::new(0, 4), (0, 1));
    w.add_listing(ztw_model::OrderSide::Sell, "battery", 2, 5_000);
    w.add_listing(ztw_model::OrderSide::Buy, "chip", 1, 7_000);
    w
}

#[allow(dead_code)]
pub fn session(world: World) -> Session {
    Session::new(SessionConfig::new(host_bin()), world)
}
