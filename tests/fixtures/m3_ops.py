# M3 管理操作双语言一致性矩阵（a1_matrix 驱动）：E 码表对账、金额显示值
# 换算、borrow / repay / buy 关键码路径与资金线。与 m3_ops.js 逐行对照，
# 日志序列与世界终态哈希必须一致。
#
# 世界前提（demo_world）：12×8 无墙、1 机器人、1 货架 (6,3)、1 充电桩
# (2,5)、1 装卸位、金币 200、无欠款。

import math as _m


def loop():
    # E 码表对账：键集合排序后逐字一致（codes::ALL ↔ 两份 bootstrap）。
    Game.log("e:" + ",".join(sorted(Game.E.keys())))

    # 金额线统一以 milli 记录（floor(x*1000+0.5)，与绑定层换算同式）。
    def milli(x):
        return _m.floor(x * 1000 + 0.5)

    def funds():
        Game.log("f:" + str(milli(Game.gold)) + "," + str(milli(Game.debt)))

    # 借款：基本路径、非正数 / 非数值 / 超安全范围拒绝、超额拒绝。
    Game.log("b1:" + Game.borrow(50.5))
    funds()
    Game.log("b2:" + Game.borrow(0.1))  # 浮点陷阱值：0.1*1000 两侧同误差
    Game.log("d:" + str(milli(Game.debt)))
    Game.log("b3:" + Game.borrow(0))
    Game.log("b4:" + Game.borrow(-1))
    Game.log("b5:" + Game.borrow("x"))
    Game.log("b6:" + Game.borrow(1e30))
    Game.log("b7:" + Game.borrow(2000))  # 超信用额度

    # 还款：部分归还、超额钳定到欠款。
    Game.log("r1:" + Game.repay(0.1))
    Game.log("r2:" + Game.repay(100000))
    funds()

    # 商店：成功路径与错误码矩阵（放置 → 资金次序）。
    Game.log("y1:" + Game.buy("shelf", 5, 5))
    Game.log("sh:" + str(len(Game.shelves())) + "," + str(Game.shelves()[0].capacity))
    Game.log("y2:" + Game.buy("robot", 6, 6))  # 余额 24.9 < 650
    Game.log("b8:" + Game.borrow(700))  # 借款解围
    Game.log("y3:" + Game.buy("robot", 6, 6))
    Game.log("rb:" + str(len(Game.robots())) + "," + str(Game.robots()[1].energy))
    Game.log("y4:" + Game.buy("dock", 0, 0))  # 边界格但无墙 → NOT_ON_WALL
    Game.log("y5:" + Game.buy("crane", 1, 1))  # 未知 kind
    Game.log("y6:" + Game.buy("charger", 2, 5))  # 既有充电桩占格
    Game.log("y7:" + Game.buy("shelf", -1, 5))  # 越界
    Game.log("y9:" + Game.buy("charger", 7, 7))  # 余额 74.9 < 300
    Game.log("b9:" + Game.borrow(300))  # 恰好补到信用额度
    Game.log("b10:" + Game.borrow(1))  # 额度已满
    Game.log("y10:" + Game.buy("charger", 7, 7))
    Game.log("ch:" + str(len(Game.chargers())))
    Game.log("r3:" + Game.repay(50))
    Game.log("x1:" + Game.destroy(Game.shelves()[1].id))  # 销毁新购货架（半价退款）
    Game.log("sh:" + str(len(Game.shelves())))
    funds()
