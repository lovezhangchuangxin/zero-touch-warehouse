# A1 引擎与工程结论（Python 宿主）

- 日期：2026-09-14；平台 macOS 15 / Apple M4（arm64）；Windows 未本地实测
  （首次推送后由 CI macos-latest + windows-latest 常跑——2026-09-14 评审
  修正：此前"自 A1 第 2 步起常跑"的说法超前于事实，A1 提交当时尚未推送）。
- 钉版：CPython 3.13.15 + python-build-standalone 20260901（install_only
  变体，三平台 sha256 见 scripts/fetch-python.mjs）；PyO3 0.29.2。

## 量测摘要（详见 records/a1-measurements-macos-aarch64.md，本地生成物）

| 指标 | Python | JS 基线（a0） | 备注 |
| --- | --- | --- | --- |
| 冷启动 spawn+init p50 | 21ms | ~2.4ms | 解释器初始化主导；远优于 docs 03「数百毫秒级」目标 |
| 热重载（命名空间重建）p50 | 5ms | ~2.4ms | Runtime 重建 vs 命名空间重建，同量级 |
| 变更型/写往返 p50 | 4µs | 4µs | 协议层相同，绑定层无额外开销 |
| 整 tick 1 台 p50 | 365µs | 263µs | +39%，预算内 |
| 整 tick 5 台 p50 | 838µs | 625µs | +34% |
| 浸泡 1000 次热重载 | p50 5ms / max 6ms | p50 0ms / max 1ms | 延迟平稳无趋势（内存趋势细查属发布前压测） |

配额校准建议：512MB 默认上限约为裸解释器 + 绑定层基线（数十 MB 量级）
的十倍余量，维持 docs 03 参考值；OBJ 域接管（放弃 pymalloc）的分配变慢
在整 tick 数字中不可见（µs 级），无需回调 pymalloc。

## 白名单传递依赖实测清单（narrow.rs TRANSITIVE 的依据）

- collections → itertools（洗除）、keyword、operator、reprlib、
  _collections、_collections_abc、collections.abc
- decimal → numbers、_decimal（C 加速；纯回退 _pydecimal 为死条目已
  清——_decimal 恒在）、_contextvars
- functools → types、_functools、_collections_abc；json → json.*、_json
- typing → **sys**（顶层 import，与收窄互斥→typing 移出白名单）；
  copyreg；_typing 随之清除（死条目）；abc 由 numbers 引入
- heapq/bisect → _heapq/_bisect；cmath/math 为内建模块（经
  BuiltinImporter 解析——meta_path 不能整链替换）
- 保留在 sys.modules 的机器私有项（逐项实证后收敛）：**仅 _io 与
  _warnings**——_io 是 _bootstrap_external.get_data 读源码的惰性硬依赖，
  _warnings 是 C 层告警路径的取模块入口；frozen importlib 链 / _imp /
  _codecs 不依赖 sys.modules 条目（import 机器持模块对象引用），且
  `import _imp` 会开放无审计的原生库加载原语（create_dynamic），一并
  洗除。_io 的文件面由审计钩子以只读 + 前缀内封住。
- 刻意排除：itertools / re（不可中断纯 C 入口，靠洗除+finder 双拒）、
  weakref（顶层依赖 sys + itertools，故 functools.singledispatch 受限
  ——触发时得到白名单可读错误）、fractions（依赖 re）、random（独立
  派生流待定）、time/os/io/sys/subprocess/socket 等

## 收窄加固（2026-09-14 评审落地）

- sys.modules 下划线整类保留改为显式最小清单：_signal（防 SIG_IGN 拆
  看门狗注入）、_thread（真线程破坏 IPC 帧流）、__main__（真 builtins
  字典旁路）等一律拒绝。注意 pyo3 `PyCode::run` 无条件
  `PyImport_AddModule("__main__")` 兜底会把洗除的 __main__ 重建回来，
  执行须改走 `PyEval_EvalCode`（runtime-py eval_in）。
- 真 builtins 源头拆除 open/input/print/breakpoint（eval/exec/compile
  保留——namedtuple 运行期依赖，且沙箱内仅纯计算）；sys 拆
  stdin/stdout/__stdin__/__stdout__（IPC 帧流；stderr 留诊断）。
- 绑定层执行进独立引导命名空间，玩家命名空间只发布 Game / Position /
  GameError——bootstrap 内部符号（M/GEN/_ipc 等）不再泄漏（对齐 JS 侧
  IIFE 封装）。已知残余：collections._sys 等属性链仍可达真 sys/builtins
  模块对象（危险入口已在源头拆除，只读面可见）；`import __ztw` 可达
  内部桥（服务端逐 op 全量校验，绕过增量回放属自伤面）。
- 审计钩子 open/open_code 事件：路径规范化（拒 .. 分量、防同级前缀
  碰撞）+ 写模式一律拒绝（mode 含 w/a/x/+ 或 flags 访问位非只读）。

## 构建与分发结论

- PBS install_only 变体可用：macOS libpython3.13.dylib 为 @rpath 安装名
  （build.rs 加 -rpath 指向发行物 lib）；Windows 需把 python313.dll 等
  复制到 target 目录（DLL 搜索顺序）；PYTHONHOME 由构建期 ZTW_PY_HOME
  烧入（发布打包时随发行物重定位，属原型 D 收尾）。
- .cargo/config.toml 的 [env] PYO3_CONFIG_FILE 固定指向
  target/python/pyo3-config.txt：任何 cargo 编译前需要
  `just python-dist`（幂等）；CI 已加缓存与 fetch 步骤。
- install_only 含 pip 与 site-packages：sys.path 清理 + 白名单后玩家
  不可达；发布工程（原型 D）评估裁剪以缩小体积（约省 10-15MB）。

## 双语言语义差异备忘（docs 03/08 的实证）

| 语义 | JS | Python |
| --- | --- | --- |
| 运行时内中断 | 引擎中断不可捕获 → 脚本级 INTERRUPTED 一步到位 | KeyboardInterrupt 可捕获 → 吞掉由主进程看门狗收场（宿主终止级） |
| 内存超限 | 环境级（Runtime 销毁重建） | 脚本级（解释器与命名空间保留，热重载恢复） |
| 诊断文案 | “循环中捕获所有异常”不适用（中断不可捕获） | 恰好适用（可捕获）——a0-engine-findings 残余 3 落地 |
| 写 Position 进 memory | A0 即有线值 bug（矩阵抓出，两语言同步修复） | — |
| to_dict 的数字样键 | 原生对象重排（1,2,10,02） | dict 保插入序——有序枚举对账以 map_keys 为准 |
