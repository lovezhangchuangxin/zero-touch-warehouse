# 代表性负载（一台机器人）：每 tick 移动 + r.memory 记账 + 根计数。
# 与 demo_one.js 语义逐项等价（A1 双语言对照锚点）。
def loop():
    r = Game.robots()[0]
    r.memory["last"] = Game.tick
    east = Game.tick % 2 == 0
    w = Game.map_size()[0]
    if east and r.pos.x < w - 2:
        r.move(Game.EAST)
    if not east and r.pos.x > 1:
        r.move(Game.WEST)
    Game.memory["n"] = (Game.memory["n"] if "n" in Game.memory else 0) + 1
