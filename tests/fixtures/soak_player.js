// 浸泡负载：每次热重载重建 Runtime；loop 用受控 memory + 日志（JS 侧
// 对照，语义见 soak_player.py）。
export function loop() {
  const payload = JSON.stringify({ tick: Game.tick, sqrt: Math.sqrt(Game.tick + 1) });
  Game.memory["payload"] = payload;
  Game.memory["n"] = (Game.memory["n"] || 0) + 1;
  Game.log("soak", payload);
}
