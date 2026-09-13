//! A1 Python 宿主集成测试公共辅助。

use ztw_api::harness::{Session, SessionConfig};
use ztw_model::Position;
use ztw_sim::World;

pub fn host_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ztw-host-py")
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
