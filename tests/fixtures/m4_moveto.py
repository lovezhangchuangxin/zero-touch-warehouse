# M4 find_path / move_to 双语言一致性矩阵（m4_moveto.rs 驱动）：E 码表
# 对账、寻路三分支、move_to 码路径、跨 tick 缓存推进与对象目的地解析。
# 与 m4_moveto.js 逐步对照，日志序列、终态 memory 与世界哈希必须一致。
#
# 世界前提（demo_world）：12×8 无墙、1 机器人 (1,1)、货架 (6,3)、充电桩
# (2,5)、装卸位 (0,4)-(0,5)。

import json


def loop():
    t = Game.tick
    r = Game.robots()[0]

    def xy(p):
        return "%d,%d" % (p[0], p[1])

    if t == 0:
        # E 码表对账：键集合排序后逐字一致（codes::ALL ↔ 两份 bootstrap）。
        Game.log("e:" + ",".join(sorted(Game.E.keys())))

        # find_path 三分支（空值判断须显式三分支——Python 的空列表为 falsy）。
        p1 = Game.find_path([1, 1], [6, 1])
        Game.log("p1:%d,%s,%s" % (len(p1), xy(p1[0]), xy(p1[-1])))
        p2 = Game.find_path([1, 1], [2, 1], {"range": 1})  # 已在到达范围
        Game.log("p2:" + ("null" if p2 is None else "len%d" % len(p2)))
        p3 = Game.find_path([1, 1], [0, 4])  # 装卸位锚点格（障碍）range 0
        Game.log("p3:" + ("null" if p3 is None else "len%d" % len(p3)))
        # 对象目的地：货架（障碍格，range 1 到达集取邻格）。
        p4 = Game.find_path(r.pos, Game.shelves()[0], {"range": 1})
        Game.log("p4:" + ("null" if p4 is None else "len%d,%s" % (len(p4), xy(p4[-1]))))
        # 受控序列可作坐标（docs 08：「凡接受坐标的参数同样接受普通或受控的
        # 两元素序列」——双语言对账的关键形态）。
        Game.memory["wp"] = [2, 1]
        p5 = Game.find_path(Game.memory["wp"], [4, 1])
        Game.log("p5:" + ("null" if p5 is None else "len%d" % len(p5)))

        # move_to 码路径：ARRIVED 不占行动 → 随后仍可受理；重复提交拒。
        Game.log("m1:" + r.move_to([2, 1]))  # 距离 1 ≤ 默认 range 1
        Game.log("m2:" + r.move_to([5, 1]))  # OK，占用行动机会
        Game.log("m3:" + r.move_to([5, 1]))  # ALREADY_ACTED（缓存原样保留）
        # 保留键 _move 由 move_to 管理，玩家写入拒绝。
        try:
            r.memory["_move"] = 1
        except GameError as e:
            Game.log("wm:" + e.code)
    elif 1 <= t <= 5:
        # 跨 tick 缓存推进：t1 换目标触发重寻，之后沿缓存轨迹走，t5 到达。
        code = r.move_to([6, 1], {"range": 0})
        Game.log("w%d:%s@%s" % (t, code, xy(r.pos)))
    elif t == 6:
        # 缓存玩家可读（JSON 字符串标量；跨 tick / 重载持久）；w2–w4 沿
        # 缓存走、不重写。
        mv = json.loads(r.memory["_move"])
        Game.log("mc:%s|%s|%d" % (xy(mv["goal"]), mv["range"], len(mv["path"])))
        # 越界目标 → NO_PATH（不占行动机会；放在 md 前以免被行动占用短路）。
        Game.log("np:" + r.move_to([99, 99]))
        # 对象目的地（装卸位锚点格为障碍，range 1 到达集取邻格）。
        Game.log("md:" + r.move_to(Game.docks()[0]))
