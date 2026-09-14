# Python 入门示例：移动 + 受控 memory 记账 + 日志 + 接单查询。
# JS 对照见 demo_one.js；游戏 API 同名同语义（docs/game-design/08）。

Game.memory["n"] = 0


def loop():
    r = Game.robots()[0]
    r.memory["last"] = Game.tick
    east = Game.tick % 2 == 0
    w = Game.map_size()[0]
    if east and r.pos.x < w - 2:
        r.move(Game.EAST)
    if not east and r.pos.x > 1:
        r.move(Game.WEST)
    Game.memory["n"] = Game.memory["n"] + 1
    if Game.tick % 10 == 0:
        Game.log("tick", Game.tick, "gold", Game.gold)
