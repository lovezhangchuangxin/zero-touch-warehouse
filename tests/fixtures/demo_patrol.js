// 巡逻示例（A0 第一步）：一台机器人东西往返。
// 查询走本地镜像（无 IPC）；move 走同步 IPC 受理。
let dir = Game.EAST;

function loop() {
  const r = Game.robots()[0];
  if (r.last_result && r.last_result.code !== Game.E.OK) {
    Game.log("settle-fail", r.id, r.last_result.code);
  }
  const w = Game.map_size()[0];
  if (r.pos.x >= w - 2 && dir === Game.EAST) dir = Game.WEST;
  if (r.pos.x <= 1 && dir === Game.WEST) dir = Game.EAST;
  const code = r.move(dir);
  if (code !== Game.E.OK) {
    Game.log("accept-fail", code);
  }
}
