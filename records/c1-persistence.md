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
- 设置自 localStorage 迁移属**有意不搬迁旧数据**（原型期本地开发，docs 06「1.0 之前不做迁移」同款策略）。

## 合入后审查轮（三路独立子代理代码审查）

审查覆盖 sim+api 数据面 / desktop 管线 / web 前端，分级发现与处置：

- **desktop P1-1 自动档轮换失效（已修）**：`ticket_auto` 在信封顶层读 `created_at_ms`，字段实在 `save` 段内——恒取不到、最旧比较永假，三槽满后永远覆写 auto-1。修正取值路径；轮换测试断言收紧为精确集合 {2,3,4}（原断言放行了槽 1 固钉式假轮换）。
- **desktop P1-2 写路径未串行（已修）**：序号门 check-then-IO 存在 TOCTOU，同目标共享同一 `.tmp` 名可交错损坏最终档。门锁改为贯穿新鲜度检查到原子替换（写入任务串行的字面实现）；`write_json_atomic` 的 tmp 名掺全局计数器。新增并发写回归测试（同目标双票并发，终态完好、新序号在场、无残留 tmp）。
- **web P1×3（已修）**：回主菜单自动存档被自身门控短路（watch 触发时 menuOpen 已 true，从未执行）；`applyDrafts` 把旧对局编辑器状态按复用的 `main:*` id 塞回缓存，跨语言读档时非活动语言草稿被旧内容顶替并落盘固化（一行删除即修）；存档名输入框 Enter 未防 IME 组合输入。
- **P2 处置**：已修——summarize 灰条兜底同根因修正、Windows 盘符相对路径防御（拒绝含 `:` 的 id）、读档初始化失败附带故障码、load_game 继承当前会话 cfg、设置序列化错误不再静默写空文件、memory 恢复期键唯一性 / robots 键规范十进制 / `_move` 保留键拒绝、六动作产出侧码表测试、去抖尾部丢失补排、卸载清定时器、读档失败反馈回暂停浮层、`inGame` 失败回滚、SaveSlots 管理场景隐藏载入按钮。未修（记录在案）——encode 对不可序列化值 panic（合法路径不可达）、from_snapshot 跨字段一致性校验（需绕过校验和 + 指纹双重对账的手改档专属）、残留 tmp 清扫、同毫秒双手动存档的先到者丢弃语义。
- 审查结论基线：sim+api 无 P0/P1（JSON 往返确定性经 serde_json 1.0.151 边界值独立实证：u64::MAX、2^63、-0.0、次正规数位级稳定）；desktop 安全点语义、临时会话流程、Tauri 参数映射、Windows 兼容逐项核对通过。
- 回归：`cargo test --workspace` 286 passed / 0 failed；clippy `-D warnings` 零告警；web 四项通过。
