// memory 写基线：每 tick 一次受控根写 + 一次 r.memory 写。
Game.memory["i"] = 0;
function loop() {
  Game.memory["i"] = Game.memory["i"] + 1;
  Game.robots()[0].memory["m"] = Game.tick;
}
