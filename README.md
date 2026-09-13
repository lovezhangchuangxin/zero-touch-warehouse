# 无人仓库（Zero-Touch Warehouse）

单机 2D 编程游戏：玩家写代码运营自动化仓库。设计文档见 `docs/`（语义以文档为准）。

## 当前状态：原型 B1（模拟核心补全）

在 A0 地基（独立宿主进程、同步 IPC、失控终止、受控 memory、查询镜像）
之上补全模拟核心：电力与充电、take / give / pick / drop 与被动转交、
车辆离场 → 订单完成 → 收款 → 装卸位释放闭环、market.cancel 与最小
destroy、xoshiro 分流 PRNG（装卸位分配），配齐原型 B 模拟回归清单
（含 ≤5 台全排列确定性）。无 UI、无 Python、无存档（分别属 B2 / A1 / C）。

## 布局

| 路径 | 内容 |
| --- | --- |
| `crates/model` | 实体、坐标、结果码、memory 线值（纯数据） |
| `crates/sim` | tick 状态机、六动作受理与三段式结算、市场与车辆生命周期、PRNG |
| `crates/api` | Game 门面、查询镜像、受控 memory、IPC 协议、绑定层（bootstrap.js）、headless 测试 harness |
| `crates/runtime` | JS 宿主进程二进制 `ztw-host-js`（rquickjs / quickjs-ng） |
| `tests/fixtures` | 玩家示例与故障注入脚本 |

## 构建与测试

```sh
cargo test --workspace          # 全部验收测试（含进程级故障注入）
cargo test -p ztw-runtime --test a0_measure -- --nocapture   # 量测（写入 records/）
```

量测与引擎行为记录在 `records/`（本地生成物，不入库；见该目录下引擎结论文件）。

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
（`just lint` 快检 / `just gate` 完整门禁 / `just dev` 桌面开发模式）。
pnpm 建议经 corepack 启用，版本由 `web/package.json` 的 `packageManager` 钉死，
与 CI 一致。

## 已知边界

- 快速引擎结论（中断不可捕获、OOM 可捕获等）见 `records/a0-engine-findings.md`。
- 伪造 InternalError 可触发环境重建（误用自伤，非安全边界）。
- 平台：当前仅 macOS arm64 实测；Windows 构建冒烟属原型 D。
- B1 code review 遗留技术债（低危，后续随相关里程碑处理）：take-from-robot
  结算草稿不更新被动方 carry（transferred 封锁使其当前无害）；挂单 qty 无
  上限校验（构造场景限定）；退款加法未统一 saturating；JS pick/drop 缺参
  静默视为 (0,0)；销毁已预留装卸位复用 HAS_VEHICLE 码；满电充电返回 OK
  且增益 0；世界不变量断言器不校验悬挂引用（docked_vehicle 等）。
