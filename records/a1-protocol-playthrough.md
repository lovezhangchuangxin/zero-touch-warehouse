# A1 验收记录：完整协议与双语言

- 日期：2026-09-14；提交：本文件随 A1 第 5 步提交入库
- 平台：macOS 15（arm64，Apple M4）；构建 debug；rustc 1.96
- 运行时：quickjs-ng（rquickjs 0.13）+ CPython 3.13.15+20260901（PyO3 0.29）
- 命令：`cargo test --workspace`（CI 同款门禁）

## docs/architecture/08 验收对照（A1 行）

| 验收项 | 落点（测试） | 结果 |
| --- | --- | --- |
| IPC 故障注入：提交前断连 | `a1_protocol::disconnect_before_send_leaves_request_uncommitted` | 通过：take 已提交、未送达 log 不落地、重启恢复 |
| IPC 故障注入：提交后回复前断连 | `a1_protocol::disconnect_after_send_before_reply_is_consistent_and_not_replayed` | 通过：竞态两分支状态一致（扣款=订单数）、重启不重放（take_if_missing 查询后接单） |
| IPC 故障注入：回复后断连 | `a0_faults::crash_between_take_and_bookkeeping_host_abort`（A0 已有，v2 下复验通过） | 通过 |
| 重复请求不重复执行 | `a1_protocol::duplicate_request_replays_cached_result_without_reexecution`；Python 侧 `a1_matrix::py_host_duplicate_request_deduped` | 通过：同号同负载重发原结果，扣款恰好一次 |
| 异负载同号拒绝 | `a1_protocol::duplicate_request_with_different_payload_is_protocol_fault` | 通过：DUP_REQUEST_MISMATCH 终止宿主 |
| 旧 epoch 拒绝 | `a1_protocol::stale_epoch_request_kills_host`（含跨代次复验）；`a1_matrix::py_host_stale_epoch_rejected` | 通过：STALE_EPOCH，被拒请求不执行 |
| 已关闭执行拒绝 | `a1_protocol::old_execution_request_rejected_as_closed` | 通过：EXEC_CLOSED 错误结果、不执行、宿主可恢复 |
| 正常完成与强制关闭竞态只结算一次 | `a1_protocol::duplicate_complete_frame_settles_once`、`late_kill_after_complete_does_not_resettle` | 通过：残留完成帧按旧执行丢弃；迟到强杀不回滚不二次结算 |
| 请求上限暂停 | `a1_protocol::request_limit_pauses_execution` | 通过：REQUEST_LIMIT 脚本级暂停、可恢复、按执行重计 |
| memory 值模型全量、两语言同结果 | `a1_matrix::memory_value_model_matrix_same_outcome_both_languages` | 通过：日志序列（ok/err:<code>/值）与终态树逐项一致——别名深拷贝、循环拒绝、嵌套写入、失效句柄、字符串数字键插入序（10,1,02,2）、±(2^53-1) 边界、Position/普通数组往返 |
| 热重载浸泡 1000 次 | `a1_soak::soak_1000_hot_reloads_{js,py}` | 通过：JS p50 0ms / max 1ms；Python p50 5ms / max 6ms（命名空间重建，模块缓存与句柄路径）；终态闭环可跑、宿主存活 |
| 能力收窄可读错误 | `a1_py_narrowing::denied_imports_give_readable_errors`、`whitelisted_modules_work_with_transitive_deps` | 通过：os/itertools/re/random/time/sys/subprocess/socket 与 open 全部可读拒绝；白名单内含传递依赖可用 |

## 矩阵测试抓出的既有缺陷（A0 遗留）

- bootstrap.js 与 bootstrap.py 的 Position 写入路径返回裸数字数组
  （`{"List":[0,-1]}`）而非线值包装（`{"List":[{"Num":0},{"Num":-1}]}`），
  主进程解码必拒。A0 测试未覆盖“写 Position 进 memory”，A1 矩阵
  “Position 往返”项首次触发；两语言同步修复（提交于 A1 第 5 步）。

## 协议语义裁决记录（实现即文档）

- request_id 宿主进程内单调、跨执行不复位；去重缓存按“当前执行内”分域
  （缓存随执行清空）。跳变/重复判定基于“已见请求号”（含被拒请求），
  被拒请求的拒绝结果同样入缓存保证幂等——否则超限后的下一执行首个
  请求会被误判 REQUEST_ID_GAP。
- 完成帧与“已见号”对账（非“已执行号”）：被拒请求见过但未执行。
- 旧执行消息以 EXEC_CLOSED **错误结果**拒绝（宿主可恢复）；旧代次帧
  属协议破坏，终止宿主。
- REQUEST_LIMIT 归脚本级：超限后继续应答错误而不执行，直到宿主自行走
  到终态帧；宿主“正常完成”的结局改判为超限暂停。

## Python 宿主引擎结论（对应 records/a0-engine-findings 的 JS 侧）

- `PyErr_SetInterruptEx` 只在信号存在 Python 级处理器（非 SIG_DFL/
  SIG_IGN）时才注入（signalmodule.c）；pyo3 auto-initialize 走
  `Py_InitializeEx(0)` 不装信号处理器——必须显式导入 `_signal` 注册
  default_int_handler，否则注入静默无效（第三层看门狗兜底，表现为
  “中断失效”）。
- MemoryError 归脚本级（docs 03）：解释器与命名空间保留；但命名空间
  可能钉住接近配额的数据——恢复靠热重载（GIL 内 drop 旧命名空间 +
  显式 GC，GIL 外 drop 走延迟回收时机不可控）。
- 首次限额拒绝后一次性 +16MB 诊断余量：MemoryError 的构造与展开本身
  需要分配头寸；玩家最多再挤出 16MB，有界。
- 能力收窄的坑（按发现顺序）：审计一刀切拒 open 会拦住 import 系统读
  标准库（改为只放行发行物前缀）；meta_path 整链替换会杀 BuiltinImporter
  （cmath 等内建模块解析失败→改为门卫前置）；sys.modules 缓存绕过
  finder（os/time/sys 等启动期模块须洗除）；import 系统写 __pycache__
  触发审计（dont_write_bytecode=True）；typing 顶层 `import sys` 与收窄
  互斥（移出白名单，注解语法本身不受影响）。
