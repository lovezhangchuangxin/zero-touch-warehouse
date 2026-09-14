# A1 双语言 memory 值模型矩阵（docs/architecture/08 验证表「memory 值模型」
# 行）。每步以相同字符串记录结果（ok / err:<CODE> / 值），供 Rust 侧逐条
# 对账——与 matrix_memory.js 必须产出完全相同的日志序列与终态。


def step(name, fn):
    try:
        fn()
        Game.log("ok:" + name)
    except Exception as e:
        Game.log("err:" + name + ":" + getattr(e, "code", type(e).__name__))


alias = [1, 2]
Game.memory["alias"] = alias  # 写入深拷贝
alias.append(3)  # 陷阱：原生列表后续变化不影响 memory
Game.memory["alias"].append(4)  # 受控句柄追加 → 提交

Game.memory["nest"] = {"a": {"b": [1]}}
Game.memory["nest"]["a"]["b"].append(2)
Game.memory["nest"]["a"]["c"] = 5

# 失效句柄：持有 nest.a 后整体替换 nest，旧句柄操作必须被拒。
h = Game.memory["nest"]["a"]
Game.memory["nest"] = 1
step("stale", lambda: h["b"].append(3))

# 字符串数字键：插入序保持（"10"/"1"/"02"/"2" 不得按数值重排）。
Game.memory["k"] = {}
Game.memory["k"]["10"] = "a"
Game.memory["k"]["1"] = "b"
Game.memory["k"]["02"] = "c"
Game.memory["k"]["2"] = "d"

# 整数与数值边界（docs 06 值模型：±(2^53-1)）。


def _big_ok():
    Game.memory["big"] = 9007199254740991


def _big_over():
    Game.memory["big"] = 9007199254740992


def _big_neg_ok():
    Game.memory["big"] = -9007199254740991


def _nonfinite():
    Game.memory["nf"] = float("inf")


def _cycle():
    c = []
    c.append(c)
    Game.memory["cyc"] = c


step("big_ok", _big_ok)
step("big_over", _big_over)
step("big_neg_ok", _big_neg_ok)
step("nonfinite", _nonfinite)
step("cycle", _cycle)

# Position 与普通数组往返。
Game.memory["pos"] = Game.NORTH
Game.memory["pos2"] = [3, 4]

import json as _json


def _plain(x):
    # 线值 Num 在 Python 侧读回为 float；日志对账按规范形态渲染
    # （整数值不带 .0，序列分隔符与 JSON.stringify 一致）。
    if isinstance(x, float) and x.is_integer():
        return int(x)
    if isinstance(x, list):
        return [_plain(i) for i in x]
    return x


def loop():
    p = Game.memory["pos"]
    Game.log("pos " + str(_plain(p[0])) + "," + str(_plain(p[1])))
    Game.log("kkeys " + ",".join(Game.memory["k"].keys()))
    Game.log("alias " + str(len(Game.memory["alias"])))
    # 有序枚举对账以 map_keys 为准（docs 06：相同有序枚举）；to_dict 在
    # JS 侧受原生对象键重排语义影响，不做字符串级对照。
    Game.log("alias_all " + _json.dumps(_plain(Game.memory["alias"].to_list()), separators=(",", ":")))
