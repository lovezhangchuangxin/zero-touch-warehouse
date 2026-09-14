# 受控 memory 用法与"深拷贝陷阱"演示（docs/architecture/06）。
# 与 memory_ops_demo.js 语义逐项等价（A1 双语言对照锚点）。
Game.memory["scalar"] = 42
Game.memory["note"] = "hello"

items = []                    # 原生列表
Game.memory["items"] = items  # 写入深拷贝
items.append(1)               # 陷阱：原生列表后续变化不影响 memory

Game.memory["items"].append(2)          # 受控句柄追加 → 提交
Game.memory["nested"] = {"a": {"b": [1, 2]}}
Game.memory["nested"]["a"]["c"] = 3     # 嵌套句柄写 → 逐层提交


def loop():
    Game.memory["items"].append(Game.tick)
    Game.memory["count"] = len(Game.memory["items"])
    r = Game.robots()[0]
    r.memory["visited"] = (r.memory["visited"] if "visited" in r.memory else 0) + 1
    Game.log("keys", ",".join(Game.memory.keys()))
