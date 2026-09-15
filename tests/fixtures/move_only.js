// 变更型调用基线：每 tick 恰一次 move（东西往返）。
export function loop() {
  const r = Game.robots()[0];
  const east = Game.tick % 2 === 0;
  const w = Game.map_size()[0];
  if (east && r.pos.x < w - 2) r.move(Game.EAST);
  if (!east && r.pos.x > 1) r.move(Game.WEST);
}
