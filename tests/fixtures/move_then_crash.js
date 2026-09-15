// 配合 ZTW_FAULT=abort_after_reply:robot.move：move 回复送达后宿主硬崩溃。
export function loop() {
  Game.robots()[0].move(Game.EAST);
}
