# C1 存档与恢复验收记录

- 语义权威：`docs/architecture/06-persistence.md`（「文件格式与落盘」一节随本里程碑落定）
- 平台：macOS 15（darwin 24.6.0，arm64）实测；Windows 由 CI 双平台矩阵复核
- 门禁：`cargo test --workspace` 284 passed / 0 failed；`cargo clippy --workspace --all-targets -- -D warnings` 零告警；web 四项（oxlint / oxfmt / typecheck / build）通过

## 决策摘要

- **编码**：serde_json 信封（零新增依赖，与 IPC 同栈）+ FNV-1a 64 校验和。版本三道硬门（存档格式 v1 / 模拟规则 m3 / 协议 v4），运行时版本仅记录不拦截；不兼容存档明确报错、不迁移（1.0 前策略）。
- **快照数据面**：`WorldSnapshot` 显式镜像 DTO（不给 `World` 挂 serde；serde 派生走 sim 非默认 feature，默认构建保持零外部依赖）。漏字段三重防线：编译期穷举转换、sim 往返测试 state_hash 等价、读档运行时指纹对账。
- **memory**：`snapshot()` 线值树 + 修订号入档；`MemoryTree::from_snapshot` 重建 arena 与角色标签（根 / robots / robots\<id\>），限额与写入路径同标准整体复核；节点 id 不落盘（06 文档契约）。
- **目录与写入**：`<app_data>/saves/<场景id>/manual|auto/`；自动档轮换保留 3 份；临时文件 + fsync + 系统级原子替换 + 序号门（旧序号不覆盖新档）；`config/settings.json`（本机设置）与 `drafts.json`（草稿防丢）共用原子写工具。
- **UI 面**：主菜单「继续」（最新有效自动档）/「读档」、ESC 暂停浮层「保存进度 / 读取存档」、设置面板「存档管理」（删除、损坏档灰条）；编辑器草稿去抖 2s 落盘 + 随档保存；设置自 localStorage 迁移至 settings.json（不做旧数据兼容）。

## 测试证据

| 套件 | 数量 | 覆盖 |
| --- | --- | --- |
| `crates/sim/tests/c1_snapshot.rs` | 4 | 种子 0..8 往返 state_hash 等价（tick 0/7/60 三时机）；40+60 tick 轨迹续流等价（确定性第三套的存档版，种子 0/7/20260913）；PRNG 状态字往返与全零防御（dock/market 双流）；未知动作名按坏档拒绝 |
| `crates/api/tests/c1_save.rs` | 7 | memory 重建往返（内容相等、修订号恢复、保留键语义、续写）、限额整体拒绝、四种坏档形态；信封往返 state_hash 对账 + 草稿段；版本门禁逐项（格式 / 规则 / 协议）；校验和篡改与截断；指纹对账双保险 |
| `crates/desktop/tests/c1_save_load.rs` | 8 | 真实宿主：存→推进→读档回退（tick / memory counter / 草稿段 / 场景）；运行中存档落 tick 边界且读档精确回退；坏程序档与未知场景读档不破坏当前对局（docs 08:56）；写入安全矩阵（旧序号被取代、临时写失败不产文件、载荷篡改灰条 + 校验和拒读、路径穿越拒绝、删除）；自动档 5 写保 3 槽含最新；崩溃旋钮子进程探针（替换前崩溃旧档原样 / 替换后崩溃新档完整）；旋钮清单↔探针互锁 |

故障注入旋钮（`ZTW_SAVE_FAULT`，单一事实源 `SAVE_FAULT_KNOBS`）：
`fail_tmp_write`、`corrupt_payload`（篡改 save 段 tick 一位——JSON 仍合法、校验和必检出）、`crash_before_replace`、`crash_after_replace`（测试二进制自再执行，write 内 abort）、`stale_seq`。生产路径读一次环境变量，测试经 `SaveStore::with_fault` 注入（同二进制并行测试共享进程环境，禁 set_var）。

## 已知边界

- 关窗路径尽力而为，不保证落盘（回主菜单即存 + 5 分钟定时覆盖主要离开路径；周期为初值，随校准调整——06 文档待定节）。
- 存档不兼容即拒读（1.0 前不做迁移）；Steam Cloud 文件集合与冲突界面未定（07 文档）。
- 教学进度 / 跨场景 profile 未实现（当前无此数据；将来入档随 format_version 递增）。
