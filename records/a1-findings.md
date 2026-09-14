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
  _collections、collections.abc
- decimal → numbers、_decimal / _pydecimal（C 加速与纯回退）、_contextvars
- functools → types、_functools；json → json.*、_json
- typing → **sys**（顶层 import，与收窄互斥→typing 移出白名单）、
  copyreg、_typing；abc 由 numbers 引入
- heapq/bisect → _heapq/_bisect；cmath/math 为内建模块（经
  BuiltinImporter 解析——meta_path 不能整链替换）
- 刻意排除：itertools / re（不可中断纯 C 入口，靠洗除+finder 双拒）、
  fractions（依赖 re）、random（独立派生流待定）、time/os/io/sys/
  subprocess/socket 等

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
