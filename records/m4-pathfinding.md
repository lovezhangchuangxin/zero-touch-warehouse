# M4-A 寻路 API（find_path / move_to）验收记录

- 语义权威：`docs/game-design/08-api-design.md`「寻路」「move_to 与
  robot.memory」两节（本里程碑随实现落定原待定项：`_move` 缓存格式、
  路径过期策略、调用成本上限、可视化范围）；分层裁决见
  `docs/architecture/03-player-runtime.md:120`（find_path 走 IPC）与
  `docs/architecture/04-api-bindings.md`（move_to 复合逻辑单源）。
- 平台：macOS 15（darwin 24.6.0，arm64）实测；Windows 由 CI 双平台矩阵复核。
- 门禁：`cargo test --workspace` 321 passed / 0 failed（基线 286 + 新增
  35，含审查轮回归）；`cargo clippy --workspace --all-targets -- -D
  warnings` 零告警；web 四项（oxlint / oxfmt / typecheck / build）通过。

## 决策摘要

- **分层**：BFS 纯函数在 sim（`World::find_path`，带节点预算参数）；
  `game.find_path` 与 `robot.move_to` 均为主进程 op；**move_to 复合逻辑
  （到达判定、缓存校验、重寻、`_move` 写入、move 受理透传）在 ops.rs
  单源实现**，双语言绑定只做坐标宽进（`posOf` / `_pos_of`）与透传——
  两语言无第二实现面，一致性由矩阵测试对账兜底。
- **算法**：等代价 BFS，障碍判定复用 `statically_passable`（地面货物是
  障碍、机器人不是）；方向序 tie-break 北、南、西、东（冻结点）；到达集
  = 与 goal 正交距离 ≤ range 的可通行格，首个到达即回溯。
- **`_move` 缓存恒为 JSON 字符串标量**（解析后
  `{"goal":[x,y],"range":n,"path":[[x,y],…]}`，path 含寻路起点的完整
  轨迹）：标量替换不杀容器节点，宿主 memory 槽位缓存的结构不变量
  （「服务端对玩家树无结构性写」，bootstrap.js mapHandler 注释与
  harness.rs 不变量注释）保持成立。**容器形态曾在实现中尝试并真实触发
  了该不变量破坏**（热重载后宿主槽位缓存返回已死句柄，集成测试暴露），
  改标量后修复——该教训已写入 08 文档。
- **op 内次序**（08 文档冻结）：`NO_SUCH_OBJECT` → 到达判定（`ARRIVED`，
  优先于行动占用检查）→ `ALREADY_ACTED` 短路（不寻路不写缓存）→ 缓存
  校验 / 失效重寻（`NO_PATH` 不占行动机会）→ 写 `_move` → `accept_move`
  透传受理码。提交的就是 move 意图，**结算与 state_hash 零新增语义**，
  排列回归天然不受影响。
- **预算**：`PATH_NODE_BUDGET = 20_000` 次格子扩展（50×50 全图 2 500 格
  的 8 倍余量）；超限 `err_result("PATH_BUDGET_EXCEEDED")` 抛可读
  GameError，不与 `NO_PATH` 混淆，**零新结果码**（ARRIVED / NO_PATH 五处
  码表已预置）。
- **缓存推进**：按实际位置在轨迹中索引推进（受理成功不 pop、结算失败
  位置与索引都不变即原路重试）；失效三条件 = goal / range 变化、偏离、
  下一步被静态障碍占据；跨热重载 / 读档保留（memory 语义），读档后
  首次调用按偏离规则自愈；写入撞限额静默降级为每 tick 重寻。
- **from_snapshot 放宽**：存档中 `robots/<id>` 含 `_move` 从「坏档拒绝」
  改为原样接受（实现后存档天然含 `_move`；形状异常自愈，其余坏档校验
  不动）。协议 v4 / RULES_VERSION / SAVE_FORMAT_VERSION 均零 bump。

## 测试证据

| 套件 | 数量 | 覆盖 |
| --- | --- | --- |
| `crates/sim/tests/m4_path.rs` | 12 | 三分支返回（最短 / `[]` / None）；到达集与 range（goal 为货架 / 装卸位占地取邻格、对角格距离为 2）；tie-break 方向序确定性；绕障路径逐步相邻且不穿墙；地面货物是障碍 / 机器人不是；界外 goal；负 range；预算 1/2 截断与 3 恰达（预算只截断不改变路径）；start 在障碍格照常扩展 |
| `crates/api/src/ops.rs` 单测 | 12 | ARRIVED 优先于 ALREADY_ACTED；首次调用写缓存（含起点轨迹）+ 受理成功不提前消费 + 短路不写缓存；跨 tick 走完全程；下一步被墙挡重寻绕行；偏离重寻；结算失败（CELL_CONTESTED）缓存与修订号原样、原路重试；NO_PATH 不占行动且不写缓存；goal 变化失效；负 range 两形态（move_to 受理码 / find_path GameError）；find_path 三分支；init 期放行查询拒动作；限额降级不缓存不影响移动 |
| `crates/api/tests/c1_save.rs` | 2 改 | `_move` 存档原样接受（规范 JSON 字符串与非字符串异常形态都恢复原值）；坏档矩阵移除 `_move` 条目 |
| `crates/runtime/tests/m4_moveto.rs` | 4 | JS 贯通：全锚点（三分支 / 码路径 / 位置推进 / 缓存可读）；终态位置；**热重载保留缓存 + 换目标重寻**（c1 读到重载前缓存、c2 新目标新轨迹）；init 期 find_path 放行 / move_to 拒 |
| `crates/runtime-py/tests/m4_moveto.rs` | 1 | 双语言矩阵：JS / Python 同 fixture 七 tick，日志序列逐字一致（含 E 码表对账）+ 终态 memory（`_move` 结构）一致 + state_hash 一致 |
| `crates/desktop/tests/m4_moveto_demo.rs` | 1 | 编辑器示例 demo_moveto.js 在 B2 单机场景真实宿主 240 tick 巡逻无故障、走访多段位置、`leg` 与 `_move` 落 memory |

fixtures `tests/fixtures/m4_moveto.{js,py}` 成对（m3_ops 模式）：覆盖
find_path 三分支与空值陷阱显式判断、move_to 全码路径、跨 tick 缓存推进
（w1–w4 沿缓存走不重写——`mc` 锚点读到的轨迹即 w1 写入的完整证据）、
对象目的地（货架视图 / 装卸位视图）、`_move` 玩家写 RESERVED_KEY 拒绝。

## 已知边界

- `_move` 为 JSON 字符串：玩家直接读它是字符串（需自行解析）；主消费
  者是 move_to 自身，可视化（画布路径标注）留画布交互迭代。
- 长地图缓存挤占 memory 限额时降级为每 tick 重寻（正确性不变，性能
  退化为每次全量 BFS——50×50 地图微秒级，可接受）。
- 寻路不含动态避让（机器人不是障碍，08 文档语义）——拥堵仍由玩家代码
  与结算冲突裁决（CELL_CONTESTED 原路重试）承担。
- apiData.ts / game-types 仍手动同步（无机械锚点，技术债不变）；
  旧 demos 未迁移到 move_to（M3 校准证据联动，留教学场景里程碑）。
- PATH_NODE_BUDGET 以 50×50 地图为论证前提（原型 C 目标）；地图尺寸
  落定时回访（>141×141 的正常寻路会触预算）。

## 合入后审查轮（三路独立子代理代码审查）

审查覆盖 sim+文档 / api 层 / 绑定+前端+测试，分级发现与处置：

- **P0 极值坐标 i32 溢出（已修，两路独立实测复现）**：`(p.x - goal.x).abs()`
  对玩家可控的裸 i32 goal 在极值处回绕——debug panic、release 静默返回
  错误 ARRIVED / 空 path。修复：sim 与 ops.rs 的距离计算升 i64、BFS 邻步
  `checked_add`（越界邻格由界检查过滤，语义不变）；回归测试
  `extreme_coordinates_do_not_overflow`（i32::MIN goal / i32::MAX start /
  巨大 range 三形态）。
- **P0 受控序列双语言分歧（已修）**：08 文档承诺「凡接受坐标的参数同样
  接受普通或受控的两元素序列」，但 JS 受控列表是 Proxy（`Array.isArray`
  为 false）被 `posOf` 拒绝（INVALID_ARGUMENT），Python 正常接受。修复：
  `posOf` 补 length===2 + 数字下标判别（先于视图对象分支）；矩阵 fixture
  增补 `p5` 锚点（受控序列作 find_path start）双语言对账。
- **P1 ARRIVED 误入 AcceptFail 诊断（已修）**：`diag_accept` 只排除 OK，
  停驻目的地的机器人每 tick 产生 `accept_fail` 噪音。修复：move_to 臂
  对 ARRIVED 跳过（与 08 结果码表「OK / ARRIVED 同为成功」对齐）。
- **P1 NaN / Infinity / 超 i32 坐标形态分歧（已修）**：JS `NaN|0 === 0`
  会以 (0,y) 为真实目标静默移动，Python 裸抛 ValueError/OverflowError
  （非 GameError）。修复：两侧同口径守卫（`coordOk` / `_num_ok`——有限
  且 |v| < 2^31），非法归一为 INVALID_ARGUMENT / 可读抛错；move_to 数字
  分支同守卫。
- **P1 demo 充电链路死代码（已修文案）**：移动不耗电（docs 03），巡逻
  场景能量恒满，「低电回桩」从未执行。处置：demo 注释如实标注为扩展位
  （接入取放动作后自然接管）、示例描述与测试 docstring 同步修正；另补
  防御初始化（中途切换脚本 tick > 0 也能起步，审查 P2）。
- **P2 处置**：已修——from_snapshot 对容器形 `_move` 收紧回坏档拒绝
  （服务端恒写标量，容器即手改档；不变量恢复无例外，c1 测试同步）；
  `actor()` 改调 `has_pending_intent` 消除同谓词双拷贝；tie-break 补
  北先于南测试 + 同世界重复调用确定性测试 + 越界 start 测试；08 文档
  tie-break 措辞精确化（BFS 扩展序而非 goal 局部方向序）、`_move` 插图
  键序对齐实现产物（goal, path, range，注明 JSON 无序）；apiData
  find_path 文案补节点预算超限通道；opts 判定 JS 侧补 `typeof object`
  （与 Python dict-only 收敛）。未修（记录在案）——结算失败重试测试的
  intent 方向直接断言（现有「修订号不变 + 缓存逐字相等」论证已成立，
  扩 sim 公开面不值）；Python `_pos_of` 对 len-2 duck-typed 序列的接受
  面宽于 JS（形态空间本身跨语言不可比）；JS 类实例携带 range 属性仍会
  被透传（Python dict-only，微小不对称）。
- 审查确认无误的基线：BFS 纯函数承诺（不动世界 / 零 PRNG / 不入
  state_hash）、move_to 状态机次序与 08 文档逐条一致、decode 对 20+
  种坏形态安全自愈、服务端写路径仅绕过保留键检查、payload/codes 双
  parity、fixtures 双语言逐行对齐由三重对账锁定、热重载缓存存活测试
  真实有效、TS 声明 `--strict` 通过。
- 回归：`cargo test --workspace` 321 passed / 0 failed；clippy
  `-D warnings` 零告警；web 四项通过。
