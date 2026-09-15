// 超限 memory 写：原子拒绝、原树不变。
Game.memory["seed"] = 1;
export function loop() {
  try {
    Game.memory["big"] = "y".repeat(300 * 1024);
    Game.log("unexpected-success");
  } catch (e) {
    Game.log("mem-err", e.code);
  }
}
