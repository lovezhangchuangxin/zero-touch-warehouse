// 代表性负载（五台机器人）：移动 + 每台 memory 记账 + 根计数。
export function loop() {
  const east = Game.tick % 2 === 0;
  for (const r of Game.robots()) {
    r.memory["last"] = Game.tick;
    const w = Game.map_size()[0];
    if (east && r.pos.x < w - 2) r.move(Game.EAST);
    if (!east && r.pos.x > 1) r.move(Game.WEST);
  }
  Game.memory["n"] = (Game.memory["n"] || 0) + 1;
}
