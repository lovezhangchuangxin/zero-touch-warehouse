# 浸泡负载：每次热重载重建命名空间；loop 用白名单模块 + 受控 memory +
# 日志，覆盖模块缓存复用与句柄释放路径（docs 08「热重载浸泡」行）。
import json
import math


def loop():
    payload = json.dumps({"tick": Game.tick, "sqrt": math.sqrt(Game.tick + 1)})
    Game.memory["payload"] = payload
    Game.memory["n"] = (Game.memory["n"] if "n" in Game.memory else 0) + 1
    Game.log("soak", payload)
