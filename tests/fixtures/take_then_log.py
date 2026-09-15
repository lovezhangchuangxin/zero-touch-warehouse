# 配合 ZTW_FAULT=abort_before_send:log：take 已提交，log 在发出前 abort，
# 主进程从未见到该请求（提交前断连点）。与 take_then_log.js 等价
# （a1_fault_parity 双语言对账锚点）。
def loop():
    asks = Game.market.sell_orders()
    if len(asks) > 0:
        Game.market.take(asks[0].id)
    Game.log("after take")
