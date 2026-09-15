# 变更型调用基线：每 tick 恰一次 move（东西往返）。与 move_only.js
# 等价（a1_fault_parity 双语言对账锚点）。
def loop():
    r = Game.robots()[0]
    east = Game.tick % 2 == 0
    w = Game.map_size()[0]
    if east and r.pos.x < w - 2:
        r.move(Game.EAST)
    if not east and r.pos.x > 1:
        r.move(Game.WEST)
