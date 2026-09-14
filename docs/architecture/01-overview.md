# 技术栈与总体结构

## 选型

| 层 | 选型 | 说明 |
| --- | --- | --- |
| 外壳 | Tauri 2 | 窗口、系统对话框、Windows / macOS 打包 |
| 模拟与运行时 | Rust workspace | 世界状态、tick 状态机、双语言宿主 |
| 界面 | Vue 3 + TypeScript | 面板、代码编辑器、调试器 |
| 渲染 | Pixi.js 8，WebGL | 仓库网格视图（见 [05](05-frontend.md)） |
| Steam | `steamworks` crate | 成就、云存档（见 [07](07-steam-platforms.md)） |
| JS 运行时 | rquickjs（内嵌 quickjs-ng） | 宿主进程内运行，经 IPC 与主进程互调 |
| Python 运行时 | PyO3 + CPython（python-build-standalone） | 宿主进程内运行，经 IPC 与主进程互调 |

游戏性能需求很轻（固定网格、几百实体、无物理），选型以开发效率、编辑器生态和调试体验为先。

## 进程结构

两个原生进程，加 webview 渲染进程：

```
┌─ webview（Vue 3 + Pixi.js）─────────────────┐
│  Pixi：仓库渲染        Vue：面板/编辑器/调试器 │
└───────────┬──────────────────▲──────────────┘
      界面命令(command)    渲染快照(channel)
┌───────────▼──────────────────┴──────────────┐
│ 主进程                                       │
│  世界线程：World + memory + tick 状态机 + 受理与结算    │
│  主进程看门狗 ｜ Steam ｜ 存档 IO             │
└───────────┬─────────────────────────────────┘
      loop() 调用与变更型请求（同步 IPC）；查询走本地镜像
┌───────────▼─────────────────────────────────┐
│ 宿主进程（runtime host，可随时终止重启）        │
│  JS 或 Python 运行时、玩家命名空间、           │
│  运行时内的中断与内存限额                      │
└──────────────────────────────────────────────┘
```

| 进程 | 内容 | 终止后果 |
| --- | --- | --- |
| 主进程 | World、权威 memory、动作受理与结算、存档、Steam、界面 | 自身崩溃则按自动存档恢复（唯一的回档来源） |
| 宿主进程 | 玩家代码执行，同一时间一种语言 | 随时可杀：终止后结算已受理动作，重启并访问主进程 memory；未提交调用和普通全局变量可能丢失 |

- 主进程健康时，宿主故障不回滚已提交世界与 memory；恢复需要处理多条 API 之间的部分完成，见 [06](06-persistence.md)。
- 主进程不依赖任何语言运行时；`loop()` 执行、变更型 `Game` 调用与 memory 读写经同步 IPC 往返，数据查询经宿主本地镜像应答（见 [03](03-player-runtime.md)）；调用开销及整 tick 尾延迟由原型量测，不预设已达成的性能结论。
- 宿主进程随应用启动；热重载不重启进程，故障与切换语言才重启。

## 世界线程

| 线程 | 职责 |
| --- | --- |
| 世界线程 | 独占 `World`，串行执行 tick 状态机；等待 `loop()` 返回期间以消息循环应答宿主的变更型请求并推送镜像增量（禁止嵌套阻塞调用宿主） |
| 主进程看门狗 | 对 tick 执行计时：先通知宿主中断，宽限期后终止宿主进程（见 [03](03-player-runtime.md)） |
| Tauri 主线程 | 窗口事件、IPC、指令队列转发 |
| Steam 回调 | steamworks 自带的回调泵 |
| 磁盘写入 | 存档落盘在世界线程产出数据后异步执行（见 [06](06-persistence.md)） |

`World` 与 memory 数据树由世界线程独占，对外发布不可变快照；宿主 memory 句柄通过受控 IPC 读写。看门狗不写世界状态。

## 代码布局

Rust workspace，`sim` 与 `api` 不依赖 Tauri 与 Steam，可独立测试：

| 模块 | 职责 |
| --- | --- |
| `crates/model` | 实体、坐标、订单、结果码等纯数据类型 |
| `crates/sim` | tick 状态机、动作受理与统一结算（语义见 [Tick、动作与电力](../game-design/03-simulation-and-actions.md)）、冲突裁决、市场 |
| `crates/api` | Game 门面定义、快照视图、受控 memory 操作；绑定、IPC 协议与文档的唯一来源（见 [04](04-api-bindings.md)） |
| `crates/runtime` | JS 宿主（ztw-host-js）：rquickjs、堆限额分配器、中断、热重载、故障处理（见 [03](03-player-runtime.md)） |
| `crates/runtime-py` | Python 宿主（ztw-host-py）：PyO3 内嵌 vendored CPython、三域配额分配器、中断注入、能力收窄（见 [03](03-player-runtime.md)） |
| `crates/desktop` | Tauri 组装、宿主进程生命周期管理、Steam adapter、存档 IO |
| `web/` | Vue + Pixi 前端（见 [05](05-frontend.md)） |
