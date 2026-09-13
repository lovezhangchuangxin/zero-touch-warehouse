# 无人仓库（Zero-Touch Warehouse）

单机 2D 编程游戏：玩家写代码运营自动化仓库。设计文档见 `docs/`（语义以文档为准）。

## 当前状态：原型 A0（JS 单语言最小运行时）

验证架构中风险最高的地基：独立宿主进程、同步 IPC、失控代码终止、
受控 memory、宿主查询镜像。无 UI、无 Python、无存档（分别属原型 B / A1 / C）。

## 布局

| 路径 | 内容 |
| --- | --- |
| `crates/model` | 实体、坐标、结果码、memory 线值（纯数据） |
| `crates/sim` | tick 状态机、动作受理与三段式结算、最小市场 |
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
