# 配合 ZTW_FAULT=dup_request:2：take（第 2 号请求）收到回复后逐字节重发。
# 与 dup_take.js 等价（A1 双语言对照锚点）。
Game.memory["t"] = 0  # 请求 #1（init 阶段）


def loop():
    asks = Game.market.sell_orders()
    if len(asks) > 0:
        Game.market.take(asks[0].id)  # 请求 #2
