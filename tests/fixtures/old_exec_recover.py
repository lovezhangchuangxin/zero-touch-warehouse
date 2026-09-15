# 配合 ZTW_FAULT=old_exec_request_first：loop 首个请求被改写为旧执行号，
# 主进程回 EXEC_CLOSED 错误结果、绑定层抛 GameError；捕获后继续发正常
# 请求并正常完成——验证被拒请求号计入已见对账。与 old_exec_recover.js
# 等价（a1_fault_parity 双语言对账锚点）。


def loop():
    try:
        Game.log("first-rejected")
    except GameError:
        # EXEC_CLOSED：旧执行可恢复拒绝，吞掉继续。
        pass
    Game.log("after-reject-1")
    Game.log("after-reject-2")
