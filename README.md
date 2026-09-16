# 无人仓库（Zero-Touch Warehouse）

单机 2D 编程游戏：玩家写代码运营自动化仓库。设计文档见 `docs/`（语义以文档为准）。

## 当前状态：A1 + 里程碑 3（经济系统）+ C1（存档与恢复）+ M4-A（寻路 API）

A0（JS 最小闭环）、B1（模拟核心）、B2（桌面壳 + 前端）、D（双平台构建
冒烟）、A1（完整协议与双语言，验收证据见 `records/a1-protocol-playthrough.md`
与 `records/a1-findings.md`）、A2（多文件模块 + JS 全面 ESM 化：协议 v3
文件集、JS 内存模块加载器与入口导出契约、Python 内存 finder 与玩家模块
驱逐、async 边界防线——TLA 未完成 / 未处理拒绝 / async 入口的可读故障）
之上完成里程碑 3：市场生成器（三货物均值
回归价格、双向常驻挂单、板面刷新与不变量维护）、借贷（按 tick 复利、
信用额度）与商店购买（robot/shelf/charger/dock，装卸位朝向自边界墙
推导）；经济不变量属性测试（会计恒等式风暴 + 借贷穷举矩阵）与周转 /
囤货 / 杠杆三策略校准（`records/m3-calibration.md`，`m3-trade` 场景 +
三支示例脚本可试玩）。其上落地 C1 存档与恢复：JSON 信封 + FNV 校验和
与三道版本门禁、`WorldSnapshot` 镜像 DTO（state_hash 往返 + 读档指纹
对账）、安全点取数 + 原子写串行队列、自动档轮换与主菜单「继续」、
编辑器草稿防丢、设置迁移 settings.json（验收证据见
`records/c1-persistence.md`；不兼容旧档明确报错不迁移）。其上落地
M4-A 寻路 API：`Game.find_path`（静态障碍等代价 BFS、三分支返回、
方向序 tie-break 冻结）与 `robot.move_to`（复合移动：主进程单源受理、
`_move` 缓存按实际位置推进、结算失败原路重试、节点预算防线），双语言
绑定零语义漂移、协议与版本面零 bump（验收证据见
`records/m4-pathfinding.md`，示例「寻路巡逻（M4）」可试玩）。

## 布局

| 路径 | 内容 |
| --- | --- |
| `crates/model` | 实体、坐标、结果码、memory 线值（纯数据） |
| `crates/sim` | tick 状态机、六动作受理与三段式结算、市场与车辆生命周期、PRNG、存档快照镜像（snapshot） |
| `crates/api` | Game 门面、查询镜像、受控 memory、IPC 协议、绑定层（bootstrap.js / bootstrap.py）、会话层 harness（desktop 世界线程与测试共用）、存档信封（save） |
| `crates/runtime` | JS 宿主进程二进制 `ztw-host-js`（rquickjs / quickjs-ng） |
| `crates/runtime-py` | Python 宿主进程二进制 `ztw-host-py`（PyO3 + vendored CPython、配额分配器、能力收窄） |
| `crates/desktop` | B2 桌面壳（Tauri 2 世界线程）+ 语言切换 + 存档服务（原子写 / 轮换 / 故障注入旋钮） |
| `web/` | pnpm monorepo：Vue 3 + Pixi 8 前端与 game-types 包 |
| `tests/fixtures` | 玩家示例与故障注入脚本（.js / .py 对照） |

## 构建与测试

```sh
just python-dist                 # 首次：获取钉版本 python-build-standalone
                                 # （~25MB；cargo 任何编译都需要它）
cargo test --workspace           # 全部验收测试（含进程级故障注入与双语言矩阵）
cargo test -p ztw-runtime-py --test a1_measure -- --nocapture   # Python 量测（写 records/）
```

量测与引擎行为记录在 `records/`（本地生成物不入库；里程碑验收证据
`git add -f` 单独入库）。双语言结论见 `records/a1-findings.md`。

## 开发工作流（git hooks 与常用任务）

克隆后安装一次 hooks（git 在所有平台——含 Windows 的 Git for Windows——都用
自带 sh 执行 hook，无需额外依赖）：

```sh
just hooks                     # 或：sh scripts/install-hooks.sh（无 just 时；
                               #   Windows 用 Git Bash 执行，或直接
                               #   git config core.hooksPath .githooks）
```

- `pre-commit`：按暂存内容跑毫秒级快检——`.rs` → `cargo fmt --check`，
  `web/` → oxlint + oxfmt --check；
- `commit-msg`：校验 Conventional Commits（`feat(api): …`，scope 可逗号并列）；
- `pre-push`：完整门禁（与 CI 同款）。耗时可观，跳过一次用 `--no-verify`。

hook 只是快速反馈、可被绕过，强制门禁在 CI。常用任务见 `justfile`
（`just lint` 快检 / `just gate` 完整门禁 / `just dev` 桌面开发模式 /
`just python-dist` 获取 Python 发行物）。pnpm 建议经 corepack 启用，
版本由 `web/package.json` 的 `packageManager` 钉死，与 CI 一致。

## 已知边界

- Python 宿主引擎结论（`_signal` 前置、MemoryError 脚本级、能力收窄坑）
  见 `records/a1-findings.md`；JS 引擎结论见 `records/a0-engine-findings.md`。
- 伪造 InternalError 可触发环境重建（误用自伤，非安全边界）。
- 平台：当前仅 macOS arm64 实测；Windows 双平台 CI 自 A1 起常跑，
  签名公证与 universal 合并属原型 D 收尾。
- B1 code review 遗留技术债（低危，后续随相关里程碑处理）：take-from-robot
  结算草稿不更新被动方 carry（transferred 封锁使其当前无害）；挂单 qty 无
  上限校验（构造场景限定）；退款加法未统一 saturating；JS pick/drop 缺参
  静默视为 (0,0)；销毁已预留装卸位复用 HAS_VEHICLE 码；满电充电返回 OK
  且增益 0；世界不变量断言器不校验悬挂引用（docked_vehicle 等）。
- 界面管理操作按钮（take/cancel/buy/borrow 的 UI 面）属里程碑 4 编辑器
  完善，当前管理操作全部经玩家代码。
- apiData.ts / game-types 与绑定层仍手动同步（无机械锚点，目标态为
  crates/api 同源生成）；旧 demos 未迁移到 move_to（M3 校准证据联动，
  留教学场景里程碑）。
