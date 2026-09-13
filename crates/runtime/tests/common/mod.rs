//! A0 集成测试公共辅助。

use ztw_api::harness::{Session, SessionConfig};
use ztw_model::{OrderSide, Position};
use ztw_sim::World;

pub fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

pub fn host_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ztw-host-js")
}

#[allow(dead_code)] // 各测试二进制独立编译，未用到该助手的套件不告警
pub fn demo_world() -> World {
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_shelf(Position::new(6, 3));
    w.add_charger(Position::new(2, 5));
    w.add_port(Position::new(0, 4));
    w.add_listing(OrderSide::Sell, "battery", 2, 5_000);
    w.add_listing(OrderSide::Buy, "chip", 1, 7_000);
    w
}

#[allow(dead_code)] // 各测试二进制独立编译，未用到该助手的套件不告警
pub fn session(world: World) -> Session {
    Session::new(SessionConfig::new(host_bin()), world)
}
