# 配合 ZTW_FAULT=dup_request:1 / dup_request_corrupt:1：log（第 1 号请求）
# 收到回复后的重发注入。与 dup_log.js 等价（a1_fault_parity 对账锚点）。
def loop():
    Game.log("first")
