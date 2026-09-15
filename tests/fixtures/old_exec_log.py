# 配合 ZTW_FAULT=old_exec_request / stale_epoch / 传输层注入：loop 首个
# 请求被改写为旧执行号 / 旧代次，或首次 loop 前发送超大帧 / 挂起。
# 与 old_exec_log.js 等价（a1_fault_parity 双语言对账锚点）。
def loop():
    Game.log("hello")
