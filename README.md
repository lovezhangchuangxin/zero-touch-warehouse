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

## 已知边界

- 快速引擎结论（中断不可捕获、OOM 可捕获等）见 `records/a0-engine-findings.md`。
- 伪造 InternalError 可触发环境重建（误用自伤，非安全边界）。
- 平台：当前仅 macOS arm64 实测；Windows 构建冒烟属原型 D。
