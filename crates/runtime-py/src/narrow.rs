//! 能力收窄（docs/architecture/03「Python 宿主」能力面；docs 08 A1 验收
//! 「能力收窄」行）：`__builtins__` 白名单 + `sys.meta_path` 白名单
//! finder + audit hook + sys.path 清理 + 危险传递依赖洗除。
//!
//! 以受信 Python 片段实现（由本 crate 内嵌源码，非玩家输入）：比 FFI
//! 组装 import 钩子更少 unsafe 面。目标按 docs 03 定位——**只防误用，
//! 不宣称抵御恶意代码**。
//!
//! 白名单初版（保守纯计算集）：math、cmath、collections、heapq、bisect、
//! functools、decimal、numbers、types、json。刻意排除：
//! - typing：顶层 `import sys` 与收窄互斥（注解语法本身无需 import
//!   typing；待 PEP 649 语义或编辑器 .pyi 路线落地后重议）；
//! - itertools / re：不可中断的纯 C 入口（count/cycle、回溯匹配），
//!   只能靠主进程杀进程兜底——但 collections 的模块级实现依赖前者，
//!   故引导期预导入后从 sys.modules 洗除（见 NARROW_PY 尾部）；
//! - fractions：依赖 re（随之排除，如需可后续评审）；
//! - random：独立派生流待定（docs 03 待定节）；
//! - time / os / io / sys / subprocess 等：非纯计算，一概拒绝。

/// 白名单模块的私有传递依赖（stdlib C 加速层与纯 Python 回退；不向
/// 玩家宣传，但按名 import 也放行——纯计算，无能力面扩张）。
pub const TRANSITIVE: &[&str] = &[
    // 私有 C 加速层与纯 Python 回退
    "_collections",
    "_decimal",
    "_pydecimal",
    "_json",
    "_functools",
    "_bisect",
    "_heapq",
    "_operator",
    "_typing",
    "_contextvars",
    // 白名单模块的公共传递依赖（纯计算）
    "abc",
    "keyword",
    "operator",
    "reprlib",
    "copyreg",
];

/// 玩家可 import 的顶层模块白名单（含必要的传递依赖根）。
pub const WHITELIST: &[&str] = &[
    "math",
    "cmath",
    "collections",
    "heapq",
    "bisect",
    "functools",
    "decimal",
    "numbers",
    "types",
    "json",
];

/// 收窄片段。在解释器初始化、宿主引导（_signal / __ztw 桥等内部依赖
/// 导入）之后、玩家命名空间建立之前执行一次。
pub const NARROW_PY: &str = r#"
import builtins as _ztw_builtins_mod
import sys as _ztw_sys

# 不写 __pycache__：发行物目录保持只读语义，也避免审计钩子拦到写入。
_ztw_sys.dont_write_bytecode = True

# 预导入绑定层所需模块：必须在 meta_path 替换之前——collections 的模块级
# 实现依赖 itertools（白名单外），走原生 import 机制完成加载后再接管。
import collections.abc  # noqa: F401  绑定层需要
import math  # noqa: F401
import types  # noqa: F401
import json  # noqa: F401

_ZTW_WHITELIST = frozenset(__ZTW_WHITELIST__) | frozenset(__ZTW_TRANSITIVE__)

class _ZtwFinder:
    """白名单 finder：顶层不在白名单的 import 一律给可读错误。"""

    def find_spec(self, fullname, path=None, target=None):
        root = fullname.split('.', 1)[0]
        if root not in _ZTW_WHITELIST:
            allowed = ', '.join(sorted(_ZTW_WHITELIST))
            raise ImportError(
                f"模块 {fullname!r} 不在白名单内。可用模块：{allowed}。"
                "文件、网络与系统访问不可用（能力收窄为误用防护）。")
        return None  # 白名单内：交给后续原生 finder（builtin / frozen / path）

_ztw_sys.meta_path = [_ZtwFinder()] + _ztw_sys.meta_path

# sys.path 只留发行物内的标准库目录（去 cwd 与 site-packages：不提供 pip）。
_prefix = _ztw_sys.prefix
_ztw_sys.path = [
    p for p in _ztw_sys.path
    if p.startswith(_prefix) and 'site-packages' not in p
]

def _ztw_audit(event, args):
    _DENY = {
        'os.system', 'os.exec', 'os.fork', 'os.spawn', 'os.posix_spawn',
        'subprocess.Popen', 'ctypes.dlopen', 'ctypes.dlsym', 'ctypes.dlsym/handle',
        'socket.getaddrinfo', 'socket.bind', 'socket.connect', 'mmap.mmap',
    }
    if event in _DENY:
        raise RuntimeError(
            f"能力收窄：事件 {event!r} 被拒绝（系统/网络访问不可用；"
            "误用防护，非安全边界）")
    if event == 'open':
        # 只放行标准库自身目录的读取（import 系统需要）；其余文件一律
        # 可读错误。args = (path, mode, flags)。
        p = args[0] if args else None
        if isinstance(p, bytes):
            p = p.decode('utf-8', 'replace')
        if not (isinstance(p, str) and p.startswith(_prefix)):
            raise RuntimeError(
                f"能力收窄：拒绝打开 {str(p)[:120]!r}（文件访问不可用，"
                "仅标准库目录内部可读；误用防护，非安全边界）")

_ztw_sys.addaudithook(_ztw_audit)

# __builtins__ 白名单：核心纯函数 + 完整异常层级（自动收集）；
# 移除 open/input/print/eval/exec/compile/breakpoint/globals/locals/
# vars/delattr/help/exit/quit 等。print 走 stdout——那是 IPC 通道，必须禁。
_ZTW_CORE = frozenset((
    '__build_class__', '__import__',
    'abs', 'aiter', 'all', 'anext', 'any', 'ascii', 'bin', 'bool', 'bytearray',
    'bytes', 'callable', 'chr', 'classmethod', 'complex', 'dict', 'dir',
    'divmod', 'enumerate', 'filter', 'float', 'format', 'frozenset', 'getattr',
    'hasattr', 'hash', 'hex', 'id', 'int', 'isinstance', 'issubclass', 'iter',
    'len', 'list', 'map', 'max', 'min', 'next', 'object', 'oct', 'ord', 'pow',
    'property', 'range', 'repr', 'reversed', 'round', 'set', 'setattr',
    'slice', 'sorted', 'staticmethod', 'str', 'sum', 'super', 'tuple', 'type',
    'zip',
))
_b = _ztw_builtins_mod
_ztw_ns_builtins = {n: getattr(_b, n) for n in _ZTW_CORE if hasattr(_b, n)}
for _n in dir(_b):
    _v = getattr(_b, _n)
    if isinstance(_v, type) and issubclass(_v, BaseException):
        _ztw_ns_builtins[_n] = _v
_ztw_ns_builtins['NotImplemented'] = _b.NotImplemented

def _ztw_new_builtins():
    return dict(_ztw_ns_builtins)

# 洗除白名单外的全部 sys.modules 缓存（sys.modules 命中会绕过 finder）：
# 解释器启动时已加载的 os / io / time / importlib 等一律移除——已加载
# 模块内部的引用不受影响，只有"按名再导入"会走 finder 被拒。保留宿主
# 机器所需的受保护根与下划线私有模块（frozen importlib 等）。
_ZTW_PROTECTED = _ZTW_WHITELIST | {'builtins', '_signal', '__ztw', '__main__'}
for _m in [m for m in list(_ztw_sys.modules)
           if m.split('.', 1)[0] not in _ZTW_PROTECTED
           and not m.startswith('_')]:
    del _ztw_sys.modules[_m]
"#;

/// 执行收窄。返回 `_ztw_new_builtins` 可调用（每次建立玩家命名空间时
/// 调用，产出白名单 __builtins__ dict）。
pub fn narrow(py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    use pyo3::types::PyDictMethods;
    let code = NARROW_PY
        .replace("__ZTW_WHITELIST__", &format!("{:?}", WHITELIST))
        .replace("__ZTW_TRANSITIVE__", &format!("{:?}", TRANSITIVE));
    let globals = pyo3::types::PyDict::new(py);
    py.run(
        &std::ffi::CString::new(code).expect("收窄源码无 NUL"),
        Some(&globals),
        None,
    )?;
    Ok(globals
        .get_item("_ztw_new_builtins")?
        .expect("收窄片段导出 _ztw_new_builtins")
        .unbind())
}
