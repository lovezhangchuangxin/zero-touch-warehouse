# API 设计（当前基线）

本篇把此前"示意"的接口名称与签名升级为当前基线，实现前仍可调整。示例以 Python 书写；JavaScript 中名称与拼写完全相同，元组对应数组、`None` 对应 `null`、字典对应对象。

## 总则

1. **一切经 `Game`**：查询、管理操作、辅助工具和常量都挂在全局 `Game` 对象上，玩家只需记住 `Game`，编辑器输入 `Game.` 即可提示全部能力。唯一例外是入口函数 `loop()`。
2. **两种语言同一张接口表**：统一 snake_case，同名同拼写，文档与教程一份两用。
3. **一切对象有 `id`**：id 是日志、`Memory`、结果记录中的规范引用；`Game.get_object_by_id(id)` 解析任意 id。凡接受 id 的参数同样接受对象本身。
4. **坐标为 `(x, y)`**：原点左上，x 向右，y 向下。
5. **错误码为大写蛇形字符串**：比较使用 `Game.E` 常量（如 `Game.E.NOT_ADJACENT`），避免手打字符串出错。反馈模型沿用 [Tick、动作与电力](03-simulation-and-actions.md)：动作调用立即返回受理码，结算结果下一 tick 经 `last_result` 查询；管理操作即时生效、立即返回准确结果（见下文）。

## Game 对象

```python
# 查询（基于 tick 快照 + 已生效的管理操作，机器人动作不改变查询结果）
Game.robots()                # -> list[Robot]
Game.shelves()               # -> list[Shelf]
Game.chargers()              # -> list[Charger]
Game.ports()                 # -> list[Port]
Game.vehicles()              # -> list[Vehicle]，等待与靠泊车辆
Game.orders()                # -> list[Order]，各状态的订单
Game.objects_at(x, y)        # 该格上的对象（机器人/货架/地面货物等），空格返回 []
Game.get_object_by_id(id)    # 任意对象，无此 id 返回 None
Game.map_size()              # -> (宽, 高)
Game.gold                    # 当前金币
Game.tick                    # 当前 tick 序号

# 管理操作（即时生效，返回码即最终结果）
Game.accept(order_id)        # 接受合同
Game.buy(kind, x, y)         # kind: "robot" / "shelf" / "charger" / "port"
Game.destroy(id)             # 受约束销毁，见经营与商店

# 辅助
Game.find_path(start, goal)  # 静态障碍最短路径，含 goal 不含 start；不可达返回 None
Game.log(...)                # 输出到日志面板
Game.memory                  # 跨代码重载的持久存储，dict 语义

# 常量
Game.E                       # 错误码表：Game.E.OK、Game.E.NOT_ADJACENT、…
Game.NORTH / SOUTH / WEST / EAST
```

`find_path` 的障碍为调用时在场的静态物：边界、建筑、地面货物；不考虑机器人（动态占用），寻路与避障策略由玩家代码负责。调用成本限制待定。

## 对象与字段

| 对象 | 字段 |
| --- | --- |
| Robot | `id` `pos` `energy` `energy_max` `carry`（Box 或 None）`last_result` |
| Shelf | `id` `pos` `boxes`（list[Box]，基线容量 4）`capacity` |
| Box | `id` `goods_type` `contract_id` `location`（所在容器 id 或地面坐标） |
| Charger | `id` `pos` |
| Port | `id` `pos`（占地尺寸待定）`docked_vehicle` |
| Vehicle | `id` `kind`（"in"/"out"）`docked` `interact_pos`（装卸交互格）`order_id` `boxes` |
| Order | `id` `state` `goods` `reward` `accept_deadline` `inbound`/`outbound`（各含 `arrive`、`deadline`、`vehicle_id`）及延误失败条款字段 |

货架不区分货位：存储是"格子里最多 4 箱"的集合语义，货物以 id 寻址。

## 机器人动作

模拟层的六个动作与 API 动词一一对应：take/give 面向容器对象（货架、车辆、机器人），pick/drop 面向地面坐标（地面不是对象，每箱占一格，坐标即唯一寻址）。

```python
r.move(Game.EAST)             # 相邻格移动
r.charge()                    # 相邻充电桩充电
r.take(target, box_id)        # 从相邻容器取得指定 id 的货物，自身须空载
r.give(target, box_id=None)   # 将指定货物放入相邻容器（货架/车辆/空载机器人）
r.pick(x, y)                  # 从相邻地面格拾起该格货物，自身须空载
r.drop(x, y, box_id=None)     # 将指定货物放到相邻空地面格
```

- `target` 接受对象或 id；`box_id` 接受 Box 对象或 id。
- 交互格判定：货架/充电桩/机器人为其所在格，车辆为其 `interact_pos`，地面即该坐标格；机器人必须与之正交相邻。
- give/drop 的 `box_id` 可缺省，缺省指当前携带的货物。机器人首期单箱携带，缺省即唯一携带物；该参数为未来多箱携带预留。指定的货物不在本机器人携带中时，受理返回 `BOX_NOT_FOUND`。
- 机器人间转交为 take/give 的目标为另一机器人的情形，规则见 [Tick、动作与电力](03-simulation-and-actions.md)。
- 所有动作调用即返回受理码；结算成败下一 tick 查 `r.last_result`（含 `action`、`arg`、`code` 三字段，最终结构待定）。

## 管理操作：即时生效

接单、购买、销毁是"老板"的操作，不与机器人的同时意图竞争，因此即时校验、即时执行、立即返回准确结果，无须下一 tick 确认：

- 调用即完成资金校验扣除、放置、状态变更；返回 `"OK"` 即已生效。
- **快照纯度的唯一例外**：tick 内查询立即可见管理操作的效果；机器人动作仍然不可见，结算前查询不变。
- 机器人动作的受理基于"当前世界"（tick 快照 + 已生效管理操作 + 已受理意图）；结算时按当时世界重新校验，目标已被销毁的已受理动作失败并返回对应错误码。
- 同一 tick 内多个管理操作按调用顺序生效，如两笔 `buy` 争同一格先到先得；"结果不依赖调用顺序"的公平性原则只约束机器人动作的统一结算。
- 新购对象当 tick 即可查询并使用。

由此，原待定的建造扣费时点、即时建成、管理操作冲突顺序、充电桩销毁与充电动作冲突，均按本节确定。

## 错误码（示例清单）

最终清单随实现冻结，比较一律使用 `Game.E` 常量。

| 阶段 | 错误码示例 | 含义 |
| --- | --- | --- |
| 受理 | `ALREADY_ACTED` | 本 tick 已有受理通过的动作 |
| 受理 | `NOT_ADJACENT` | 目标不相邻 |
| 受理 | `LOADED` / `NOT_CARRYING` | 取货时已有携带物 / 交付时空载 |
| 受理 | `NOT_ENOUGH_ENERGY` | 电量不足 |
| 受理 | `TARGET_FULL` | 目标容器无空位 |
| 受理 | `BOX_NOT_FOUND` | take：目标中不存在该货物；give/drop：指定货物不在自身携带中 |
| 受理 | `OUT_OF_BOUNDS` / `CELL_BLOCKED` | 移动越界 / 目标格为建筑 |
| 受理 | `NO_SUCH_OBJECT` | id 不存在或已销毁 |
| 结算 | `CELL_CONTESTED` | 争抢同格，全部失败 |
| 结算 | `BLOCKED` | 移动链前方未离开 |
| 结算 | `CHARGER_BUSY` | 充电桩被同 tick 充电动作占用 |
| 结算 | `TARGET_GONE` | 目标被管理操作销毁 |
| 管理 | `NO_FUNDS` / `CELL_OCCUPIED` | 金币不足 / 放置格被占 |
| 管理 | `NOT_EMPTY` / `HAS_VEHICLE` | 销毁前提不满足（含货、靠泊中等） |
| 管理 | `ORDER_GONE` / `ORDER_TAKEN` | 接单时已过期 / 已被接受 |

## 示例：教学级（一箱入库）

```python
def adjacent(a, b):                            # 玩家自写工具
    return abs(a[0] - b[0]) + abs(a[1] - b[1]) == 1

def step_to(r, goal):
    path = Game.find_path(r.pos, goal)
    if not path:
        Game.log("无可达路径", goal)
        return
    (x1, y1), (x2, y2) = r.pos, path[0]
    if x2 > x1:   r.move(Game.EAST)
    elif x2 < x1: r.move(Game.WEST)
    elif y2 > y1: r.move(Game.SOUTH)
    else:         r.move(Game.NORTH)

def loop():
    r = Game.robots()[0]

    if r.last_result and r.last_result["code"] != Game.E.OK:
        Game.log(r.id, r.last_result)          # 先弄清上一 tick 为什么失败

    if r.carry is not None:                    # 满载：送进货架
        shelf = Game.shelves()[0]
        if adjacent(r.pos, shelf.pos):
            r.give(shelf)
        else:
            step_to(r, shelf.pos)
    else:                                      # 空载：去靠泊的入库车取一箱
        v = next((v for v in Game.vehicles()
                  if v.docked and v.kind == "in" and v.boxes), None)
        if v is None:
            return
        if adjacent(r.pos, v.interact_pos):
            r.take(v, v.boxes[0].id)
        else:
            step_to(r, v.interact_pos)
```

## 示例：中期（三条反馈通道并用）

```python
def loop():
    # 反应上一 tick：结算失败先留痕
    for r in Game.robots():
        res = r.last_result
        if res and res["code"] != Game.E.OK:
            Game.log(r.id, res["action"], res["code"])

    for r in Game.robots():
        if r.energy < r.energy_max * 0.2:
            r.charge()
        elif r.carry is None:
            claim_job(r)        # 玩家自写：从任务池选目标，move + take
        else:
            deliver(r)          # 玩家自写：move + give / drop

    # 管理即时生效：钱够了就加一台机器人
    if Game.gold > 600 and len(Game.robots()) < 5:
        code = Game.buy("robot", Game.memory["spawn_x"], Game.memory["spawn_y"])
        if code != Game.E.OK:
            Game.log("buy robot:", code)
```

## 待定

- `last_result` 的最终字段结构；`buy` 的 `kind` 取值表。
- `find_path` 的调用成本与每 tick 次数限制。
- `interact_pos` 与装卸口占地/交互面的关系，随装卸口尺寸一并确定。
- 是否在 `Game` 之外为常用常量提供顶层快捷别名（如裸 `E`）；基线只认 `Game`。
- 两种语言运行时细节、序列化与沙箱限制，属技术设计。
