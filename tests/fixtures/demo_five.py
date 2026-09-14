# 代表性负载（五台机器人）：移动 + 每台 memory 记账 + 根计数。
# 与 demo_five.js 语义逐项等价（A1 双语言对照锚点）。
def loop():
    east = Game.tick % 2 == 0
    for r in Game.robots():
        r.memory["last"] = Game.tick
        w = Game.map_size()[0]
        if east and r.pos.x < w - 2:
            r.move(Game.EAST)
        if not east and r.pos.x > 1:
            r.move(Game.WEST)
    Game.memory["n"] = (Game.memory["n"] if "n" in Game.memory else 0) + 1
