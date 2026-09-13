# API 设计（当前基线）

本篇把此前"示意"的接口名称与签名升级为当前基线，实现前仍可调整。示例以 Python 书写；JavaScript 中名称与拼写完全相同，元组对应数组、`None` 对应 `null`、字典对应对象。

## 总则

1. **一切经 `Game`**：查询、管理操作、辅助工具和常量都挂在全局 `Game` 对象上，玩家只需记住 `Game`，编辑器输入 `Game.` 即可提示全部能力。唯一例外是入口函数 `loop()`。
2. **两种语言同一张接口表**：统一 snake_case，同名同拼写，文档与教程一份两用。关键字参数在 JS 中以选项对象传递，如 `Game.vehicles({kind: "in"})`、`r.move_to(x, y, {range: 1})`。
3. **一切对象有 `id`**：id 为全局自增整数——创建时分配的序号，跨类型唯一、单调递增、永不复用，顺序即创建顺序，也用作机器人冲突裁决的优先级依据。id 是日志、`Game.memory`、结果记录中的规范引用；`Game.get_object_by_id(id)` 解析任意 id（跨类型唯一，故无须类型信息）。凡接受 id 的参数同样接受对象本身。
4. **坐标采用 Position 值对象**：原点左上，x 向右，y 向下，语义见下文「Position」。
5. **结果码为大写蛇形字符串**：比较使用 `Game.E` 常量（如 `Game.E.NOT_ADJACENT`），避免手打字符串出错。反馈模型沿用 [Tick、动作与电力](03-simulation-and-actions.md)：动作调用立即返回受理码，结算结果下一 tick 经 `last_result` 查询；管理操作即时生效、立即返回准确结果（见下文）。

## Game 对象

```python
# 查询（基于 tick 快照 + 已生效的管理操作，机器人动作不改变查询结果）
Game.robots()                # -> list[Robot]
Game.shelves()               # -> list[Shelf]
Game.chargers()              # -> list[Charger]
Game.ports()                 # -> list[Port]
Game.vehicles(kind=None)     # -> list[Vehicle]，占用装卸位的车辆；kind 过滤 "in"/"out"
Game.market.sell_orders()    # -> list[Order]，卖单：对方卖出，玩家买入，车送货来
Game.market.buy_orders()     # -> list[Order]，买单：对方收购，玩家卖出，车来取货
Game.my_orders()             # -> list[Order]，已接未完成
Game.ground_boxes()          # -> list[Box]，地面上的货物
Game.objects_at(x, y)        # 该格上的对象（机器人/货架/地面货物等），空格返回 []
Game.get_object_by_id(id)    # 任意对象，无此 id 返回 None
Game.map_size()              # -> (宽, 高)
Game.gold                    # 当前金币
Game.debt                    # 当前欠款（含累计利息）
Game.tick                    # 当前 tick 序号

# 管理操作（即时生效，返回码即最终结果）
Game.market.take(order_id)   # 接单：预留空装卸位，下一 tick 车到；买入即扣款
Game.market.cancel(order_id) # 取消：车辆恢复出现时状态即可，收手续费、释放装卸位
Game.buy(kind, x, y)         # kind: "robot" / "shelf" / "charger" / "port"
Game.destroy(id)             # 销毁对象或货物，见经营与商店
Game.borrow(amount)          # 借款即时到账，受信用额度限制
Game.repay(amount)           # 归还部分或全部，金额以欠款为上限

# 辅助
Game.find_path(start, goal, opts=None)  # 静态障碍最短路径，不含 start，终点为到达集内最近格；不可达返回 None
Game.log(...)                # 输出到日志面板
Game.memory                  # 主进程持有的受控映射，跨代码重载持久

# 常量
Game.E                       # 结果码表：Game.E.OK、Game.E.ARRIVED、Game.E.NOT_ADJACENT、…
Game.NORTH / SOUTH / WEST / EAST
```

`find_path` 的完整语义见下文「寻路」一节，调用成本限制见待定。

## 对象与字段

| 对象 | 字段 |
| --- | --- |
| Robot | `id` `pos` `energy` `energy_max` `carry`（Box 或 None）`memory` `last_result` |
| Shelf | `id` `pos` `boxes`（list[Box]，基线容量 4）`capacity` |
| Box | `id` `goods_type` `holder`（所在容器 id，地面为 None）`location`（Position） |
| Charger | `id` `pos` |
| Port | `id` `pos`（占地 1×2 格，坐标锚点与朝向表达待定）`docked_vehicle` |
| Vehicle | `id` `kind`（"in"/"out"）`goods_type` `interact_pos`（装卸交互格）`order_id` `boxes`（车上现存：入库=待卸，出库=已装） |
| Order | `id` `side`（"sell"/"buy"）`goods_type` `qty` `unit_price`；已接订单另有 `vehicle` 与 `port` |

货架不区分货位：存储是"格子里最多 4 箱"的集合语义，货物以 id 寻址。出库车待装数量 = 订单 `qty` − `len(boxes)`。车辆接单后出现并占用装卸位，见 [市场与交易](04-orders-and-logistics.md)。跨对象引用字段（`docked_vehicle`、`vehicle`、`port`、`holder`）一律为 id，经 `Game.get_object_by_id` 解析；`Box.location` 为该箱当前所在格，随容器移动更新。

### Position

`pos`、`interact_pos`、地面货物的 `location` 等坐标均为 Position 值对象（值类型，无 id）：

- 字段 `x`、`y`。行为等同坐标对：Python 实现为具名元组，解构 `x, y = r.pos`、下标 `r.pos[0]`、等值比较 `r.pos == (5, 5)`、作字典键均成立；JS 实现同样支持下标与展开。
- 方法：`equals(other)` 等值比较（JS 侧必须用它，数组直接 `==` 恒为 false）、`adjacent(other)` 是否正交相邻、`distance(other)` 正交步数、`neighbors()` 四个相邻格。
- 存入 `Game.memory` 转为 `[x, y]`，读回为受控序列，不自动还原 Position；凡接受坐标的参数同样接受普通或受控的两元素序列。

### memory 操作约定

`Game.memory` 及 `r.memory` 是受控数据树的访问句柄。嵌套修改同步提交到主进程；写入原生容器先深拷贝，之后修改原变量不会改动 memory。只接受字符串键；不合法值与容量超限在修改时原子拒绝。完整值模型、支持操作、失效句柄和初始化提交规则以[存档与恢复](../architecture/06-persistence.md)为准。

宿主崩溃保留已提交修改，但接单与写任务记录是两个独立操作。程序初始化及恢复时应查询已接订单、机器人携带物和动作结果，修复任务表，不可只凭 memory 假设世界状态。

## 寻路

```python
path = Game.find_path(r.pos, goal)          # opts 例：{"range": 1}
# [(x1, y1), (x2, y2), …]   最短路径，不含 start，终点为到达集内最近格
# []                        已在到达范围内，无需移动
# None                      不可达
```

- start 与 goal 接受坐标或对象；对象取坐标的规则与 move_to 一致。
- 可通行格为界内非建筑格；障碍是边界、货架、充电桩、装卸口占地与地面货物。机器人不算障碍——寻路回答"物理可达"，动态避让由玩家代码负责。
- 一切可通行格等代价，返回最短路径之一；相同世界状态下结果确定，便于复现与调试。
- `opts.range` 为到达判定半径（正交距离）：与目的格正交距离 ≤ range 的可通行格构成到达集，到达集为空即不可达；默认 0，即目的格本身须可通行。`move_to` 默认以 1 调用。其余 opts 留作扩展（如地形代价），`move_to` 原样转发。
- 纯查询，不消耗行动机会，可用于规划与试探。空值判断需显式三分支：JS 中 `[]` 为 truthy，`if (path)` 会把"已到达"当成"有路径"；Python 中 `if path:` 会把"已到达"并入"不可达"。两端都以 `path === null` / `path is None` 判不可达，以空列表判"无需移动"。

## 机器人动作

模拟层的六个动作与 API 动词一一对应：take/give 面向容器对象（货架、车辆、机器人），pick/drop 面向地面坐标（地面不是对象，每箱占一格，坐标即唯一寻址）。

```python
r.move(Game.EAST)             # 相邻格移动
r.move_to(target, opts=None)   # 复合移动：寻路并自动提交一步，见下文
r.charge()                    # 向相邻 id 最小的充电桩申请本 tick 充电
r.take(target, box_id)        # 从相邻容器取得指定 id 的货物，自身须空载
r.give(target, box_id=None)   # 将指定货物放入相邻容器（货架/车辆/空载机器人）
r.pick(x, y)                  # 从相邻地面格拾起该格货物，自身须空载
r.drop(x, y, box_id=None)     # 将指定货物放到相邻空地面格
```

- `target` 接受对象或 id；`box_id` 接受 Box 对象或 id。
- 交互格判定：货架/充电桩/机器人为其所在格，车辆为其 `interact_pos`，地面即该坐标格；机器人必须与之正交相邻。take/give 的目标须为货架、车辆或机器人，否则返回 `INVALID_TARGET`。
- drop 受理只校验静态占用；目标格被机器人占据时受理通过，结算时该机器人未离开则失败 `CELL_OCCUPIED`。
- give 到车辆须类型匹配：货物的 `goods_type` 与该车辆订单一致，受理校验，不符返回 `WRONG_GOODS`；入库车可装回同类型货物。车辆离场后从 `Game.vehicles()` 消失，对它交互返回 `NO_SUCH_OBJECT`。
- give/drop 的 `box_id` 可缺省，缺省指当前携带的货物。机器人首期单箱携带，缺省即唯一携带物；该参数为未来多箱携带预留。指定的货物不在本机器人携带中时，受理返回 `BOX_NOT_FOUND`。
- 机器人间转交为 take/give 的目标为另一机器人的情形，规则见 [Tick、动作与电力](03-simulation-and-actions.md)。
- 所有动作调用即返回受理码；结算成败下一 tick 查 `r.last_result`（含 `action`、`arg`、`code` 三字段，最终结构待定）。

### move_to 与 robot.memory

- `r.memory` 是 `Game.memory["robots"][str(r.id)]`（JS 使用 `String(r.id)`） 的快捷引用，自动创建，读写同一份数据，跨代码重载持久；其中 `_move` 为 move_to 保留键，玩家不应占用，`robots` 同为 `Game.memory` 的保留键。
- move_to 是 `find_path` + `move` 的复合封装：已到达（判定优先于行动占用检查）返回 `"ARRIVED"`，不提交动作、不占用行动机会，可当 tick 继续取放或充电；未到达则沿缓存路径提交一步 `move`，返回其受理码。所有返回值（含 `ARRIVED`）均为 `Game.E` 常量。
- 目的地接受 `move_to(x, y, opts)` 或 `move_to(target, opts)`，target 为坐标或对象：货架/充电桩/装卸口/机器人取 `pos`，车辆取 `interact_pos`，地面货物取所在格。
- 到达判定默认 `opts.range = 1`，抵达目的地或与其正交相邻即算到达——游戏内一切交互都按相邻进行；其余 opts 原样转发 `find_path`。
- 路径、目的地与 opts 缓存于 `r.memory._move`：当目的地或 opts 变化、当前位置偏离缓存路径、下一步被静态障碍（新建筑、地面货物）占据时重新寻路；结算失败（如 `CELL_CONTESTED`）不使缓存失效，下一 tick 原路重试。缓存只按实际位置推进，不在受理成功时提前消费路径。
- 目的地不可达返回 `"NO_PATH"`，不占用行动机会。

## 管理操作：即时生效

接单、取消、购买、销毁与借贷是"老板"的操作，不与机器人的同时意图竞争，因此即时校验、即时执行、立即返回准确结果，无须下一 tick 确认：

- 调用即完成资金校验扣除、放置、状态变更；返回 `"OK"` 即已生效。
- `Game.market.take` 接单即时校验并预留空闲装卸位，买入单即时扣款，车辆下一 tick 出现；多装卸口时随机占用一个空位，见 [市场与交易](04-orders-and-logistics.md)。
- `Game.market.cancel` 取消已接订单，条件与手续费见 [市场与交易](04-orders-and-logistics.md)；车辆未恢复出现时状态返回 `GOODS_MOVED`。
- `Game.borrow` / `Game.repay` 即时到账与扣减：每 tick 结算完成后按欠款 × 利率复利计入，`Game.debt` 即时可见；借款受信用额度限制，超额返回 `CREDIT_EXCEEDED`。借贷的定位见 [经营、商店与成长](06-economy-and-progression.md)。
- **快照纯度的唯一例外**：tick 内查询立即可见管理操作的效果；机器人动作仍然不可见，结算前查询不变。
- 机器人动作的受理基于"当前世界"（tick 快照 + 已生效管理操作；意图只记录本机器人行动机会，不预占共享资源）；结算时按当时世界重新校验，目标已被销毁的已受理动作失败并返回对应结果码。
- 同一 tick 内多个管理操作按调用顺序生效，如两笔 `buy` 争同一格先到先得；"结果不依赖调用顺序"的公平性原则只约束机器人动作的统一结算。
- 新购对象当 tick 即可查询并使用。

由此，原待定的建造扣费时点、即时建成、管理操作冲突顺序、充电桩销毁与充电动作冲突，均按本节确定。

## 结果码（示例清单）

最终清单随实现冻结，比较一律使用 `Game.E` 常量。

| 典型阶段 | 结果码示例 | 含义 |
| --- | --- | --- |
| 受理 | `OK` / `ARRIVED` | 成功；`ARRIVED` 为 move_to 已到达，不占用行动机会 |
| 受理 | `ALREADY_ACTED` | 本 tick 已有受理通过的动作 |
| 受理 | `INVALID_ARGUMENT` | 非法参数：buy 未知 kind、borrow/repay 金额非正数等 |
| 受理 | `NOT_ADJACENT` | 目标不相邻 |
| 受理 | `LOADED` / `NOT_CARRYING` | 取货时已有携带物 / 交付时空载 |
| 受理 | `NOT_ENOUGH_ENERGY` | 电量不足 |
| 受理 | `TARGET_FULL` | 目标容器无空位 |
| 受理 | `BOX_NOT_FOUND` | take：目标中不存在该货物；give/drop：指定货物不在自身携带中；pick：该地面格没有货物 |
| 受理 | `WRONG_GOODS` | give 到车辆：货物类型与订单不符 |
| 受理 | `INVALID_TARGET` | take/give 目标不是货架、车辆或机器人 |
| 受理 | `OUT_OF_BOUNDS` / `CELL_BLOCKED` | 坐标越界（移动/pick/drop/buy 共用）/ move：目标格为静态障碍（建筑、地面货物） |
| 受理 | `CELL_OCCUPIED` | drop/buy：目标地面格已被占用（drop 亦可出现在结算阶段） |
| 受理 | `NO_SUCH_OBJECT` | id 不存在或已销毁 |
| 受理 | `NO_PATH` | move_to 目的地不可达 |
| 结算 | `CELL_CONTESTED` | 争抢同一格失败：被 id 更小的机器人取得 |
| 结算 | `TARGET_CONTESTED` | 争抢同一货物或容器空位失败：被 id 更小的机器人取得 |
| 结算 | `CHAIN_BLOCKED` | 移动链受阻 |
| 结算 | `CHARGER_BUSY` | 同 tick 争抢充电桩失败；没有跨 tick 锁定 |
| 结算 | `TARGET_MOVED` | 交互目标的机器人同 tick 成功移动，转交失败 |
| 结算 | `TARGET_GONE` | 目标被管理操作销毁或取消移除 |
| 管理 | `NO_FUNDS` | 金币不足（buy、take 买入单、repay）；不自动借贷 |
| 管理 | `CREDIT_EXCEEDED` | borrow：超出信用额度 |
| 管理 | `ON_VEHICLE` | destroy：货物尚未卸离购入车辆 |
| 管理 | `NO_FREE_PORT` | 接单：没有空闲装卸位 |
| 管理 | `NOT_EMPTY` / `HAS_VEHICLE` | 销毁前提不满足（含货、占用中等） |
| 管理 | `ORDER_GONE` | 接单/取消：挂单已离开市场或订单已完成 |
| 管理 | `GOODS_MOVED` | 取消：车辆未恢复出现时的状态 |

## 示例：教学级（一箱入库）

```python
SPOT = (5, 5)   # 存货区的一格地面（教学第一关的订单恰好一箱）

def loop():
    r = Game.robots()[0]

    if r.last_result and r.last_result["code"] != Game.E.OK:
        Game.log(r.id, r.last_result)          # 先弄清上一 tick 为什么失败

    if r.carry is not None:                    # 满载：搬到存货区放下
        if r.move_to(SPOT) == Game.E.ARRIVED:
            r.drop(*SPOT)
    else:                                      # 空载：去入库车卸一箱
        for v in Game.vehicles(kind="in"):
            if len(v.boxes) > 0:
                if r.move_to(v) == Game.E.ARRIVED:
                    r.take(v, v.boxes[0].id)
                break
```

## 示例：中期（三条反馈通道并用）

```python
def loop():
    # 反应上一 tick：结算失败先留痕
    for r in Game.robots():
        res = r.last_result
        if res and res["code"] != Game.E.OK:
            Game.log(r.id, res["action"], res["code"])

    for i, r in enumerate(Game.robots()):
        if "charging" not in r.memory:
            r.memory["charging"] = False
        # 低电滞回：低于两成触发，充满九成解除
        if r.energy < r.energy_max * 0.2:
            r.memory["charging"] = True
        if r.energy >= r.energy_max * 0.9:
            r.memory["charging"] = False
        if r.memory["charging"] and len(Game.chargers()) > 0:
            # 按序号分桩避免争抢；移动不耗电，0 电也回得了桩旁
            charger = Game.chargers()[i % len(Game.chargers())]
            code = r.move_to(charger)
            if code == Game.E.ARRIVED:
                r.charge()
            elif code != Game.E.OK:
                Game.log(r.id, "move_to:", code)   # 受理失败也要看得见
            continue
        if r.carry is None:
            claim_job(r)        # 玩家自写：卸货/拣货，move_to + take
        else:
            deliver(r)          # 玩家自写：move_to + give / drop

    # 交易策略：有空装卸位且电池库存（含在途买入）低于下限，接最便宜的电池卖单
    stock = sum(1 for s in Game.shelves()
                for b in s.boxes if b.goods_type == "battery")
    stock += sum(1 for b in Game.ground_boxes()
                 if b.goods_type == "battery")
    stock += sum(o.qty for o in Game.my_orders()
                 if o.side == "sell" and o.goods_type == "battery")
    if stock < 4 and any(p.docked_vehicle is None for p in Game.ports()):
        asks = [o for o in Game.market.sell_orders()
                if o.goods_type == "battery"]
        if len(asks) > 0:
            best = min(asks, key=lambda o: o.unit_price)
            code = Game.market.take(best.id)
            if code != Game.E.OK:
                Game.log("take:", code)

    # 管理即时生效：钱够了就加一台机器人（出生点自地图右下角向下推进）
    if Game.gold > 800 and len(Game.robots()) < 5:
        if "spawn" not in Game.memory:
            w, h = Game.map_size()
            Game.memory["spawn"] = [w - 2, h - 2]
        x, y = Game.memory["spawn"]
        code = Game.buy("robot", x, y)
        if code != Game.E.OK:
            Game.log("buy robot:", code)
        h = Game.map_size()[1]
        Game.memory["spawn"] = [x, (y + 1) % h]   # 无论成败都推进，避免原地重试
```

## 待定

- `last_result` 的最终字段结构；`buy` 的 `kind` 取值表。
- `find_path` 与 `move_to` 的调用成本限制；`_move` 缓存格式；是否需要按 tick 的路径过期（类似 Screeps `reusePath`）；是否内建路径可视化。
- `interact_pos` 与 1×2 格装卸口占地/交互面的关系，以及坐标锚点与朝向表达。
- 市场生成参数，见 [市场与交易](04-orders-and-logistics.md)。
- 是否在 `Game` 之外为常用常量提供顶层快捷别名（如裸 `E`）；基线只认 `Game`。
- 两种语言运行时细节、序列化与沙箱限制，属技术设计。
