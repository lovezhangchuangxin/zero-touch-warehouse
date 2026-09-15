// 代表性负载（一台机器人）：每 tick 移动 + r.memory 记账 + 根计数。
export function loop() {
  const r = Game.robots()[0];
  r.memory["last"] = Game.tick;
  const east = Game.tick % 2 === 0;
  const w = Game.map_size()[0];
  if (east && r.pos.x < w - 2) r.move(Game.EAST);
  if (!east && r.pos.x > 1) r.move(Game.WEST);
  Game.memory["n"] = (Game.memory["n"] || 0) + 1;
}
