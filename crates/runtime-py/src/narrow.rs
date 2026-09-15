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
//! - weakref：顶层依赖 sys + itertools，与上述排除互斥——因此
//!   functools.singledispatch / singledispatchmethod 受限不可用（触发时
//!   得到白名单可读错误，非静默损坏）；
//! - fractions：依赖 re（随之排除，如需可后续评审）；
//! - random：独立派生流待定（docs 03 待定节）；
//! - time / os / io / sys / subprocess 等：非纯计算，一概拒绝。
//!
//! 已知残余（误用防护定位，不追捕到零）：已加载模块的属性链（如
//! `collections._sys`）仍可达真实 sys / builtins 模块对象——危险入口
//! （open/input/print/breakpoint、stdin/stdout）已在源头从这两个模块上
//! 拆除，但 sys.modules 等只读面仍可见；`import __ztw` 可达内部桥
//! （服务端对每个 op 仍全量校验，绕过绑定层增量回放属自伤面）；
//! exec/eval/compile 保留在真 builtins（collections.namedtuple 运行期
//! 需要 eval，且沙箱内它们只能做纯计算）。

/// 白名单模块的私有传递依赖（stdlib C 加速层；不向玩家宣传，但按名
/// import 也放行——纯计算，无能力面扩张）。_pydecimal / _typing 为
/// 死条目已清（前者永不加载——_decimal 恒在；后者随 typing 一并排除）。
pub const TRANSITIVE: &[&str] = &[
    "_collections",
    "_collections_abc",
    "_decimal",
    "_json",
    "_functools",
    "_bisect",
    "_heapq",
    "_operator",
    "_contextvars",
    // 白名单模块的公共传递依赖（纯计算）
    "abc",
    "keyword",
    "operator",
    "reprlib",
    "copyreg",
];

/// 收窄后仍保留在 `sys.modules` 的机器私有项（import 机器的硬需求，
/// 见 NARROW_PY 的 `_ZTW_KEEP_PRIVATE`）。玩家文件名与其冲突必须在
/// init 拒绝——注册表的驱逐按名匹配，玩家模块会顶掉真模块并永久
/// 破坏宿主的源码装载。
pub const KEEP_PRIVATE: &[&str] = &["_io", "_warnings"];

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

# 预导入玩家模块注册表所需模块：与 collections.abc 同款时序约束——必须
# 在 meta_path 替换之前以原生 import 机器完成加载。inspect / importlib.util
# 不在白名单，随后会被 sys.modules 洗除，但此处捕获的模块对象引用持续
# 有效。注意：注册表与 loader 是引导期代码但不做属性链隔离——玩家可经
# sys.meta_path[0].find_spec.__globals__ 到达本命名空间（真 builtins 的
# exec/compile 等）；这与既有的 collections._sys 属性链残余同类
#（误用防护定位，源头拆除仍是真闸门），不得在此存放更敏感的状态。
import importlib.util as _ztw_ilu  # noqa: F401
import inspect as _ztw_inspect  # noqa: F401


class _ZtwPlayerLoader:
    """内存源加载器：玩家模块命名空间显式注入受限 __builtins__ 与当代
    宿主绑定（Game 等）。CPython 在模块 dict 缺 __builtins__ 时会回填
    【真】builtins——这里显式设置是收窄的关键一步。"""

    def __init__(self, registry, fullname):
        self._registry = registry
        self.fullname = fullname

    def create_module(self, spec):
        return None  # 走默认模块创建

    def exec_module(self, module):
        src = self._registry.files[self.fullname]
        module.__dict__['__file__'] = '<player>/' + self.fullname + '.py'
        module.__dict__['__builtins__'] = _ztw_new_builtins()
        for _k, _v in self._registry.bindings.items():
            module.__dict__[_k] = _v
        exec(compile(src, '<player>/' + self.fullname + '.py', 'exec'),
             module.__dict__)


class _ZtwPlayerRegistry:
    """玩家多文件注册表：文件集（模块名 → 源码）+ 绑定注入。

    - find_spec 只认注册表内的顶层模块名；其余返回 None 交给白名单
      finder（标准库照常、白名单外仍拒）。
    - update 在每次 init 调用：先驱逐 sys.modules 里的旧玩家模块——
      解释器跨热重载常驻，缓存里的旧模块持有旧绑定与旧状态，不得
      存活到新一代环境。
    - 名字与标准库/保护名的冲突在宿主侧 init 校验拦截（PROGRAM_INVALID），
      find_spec 不再重复。
    """

    def __init__(self):
        self.files = {}
        self.bindings = {}
        self._known = frozenset()

    def update(self, files, bindings):
        for name in [m for m in list(_ztw_sys.modules)
                     if m in self._known or m in files]:
            del _ztw_sys.modules[name]
        self.files = dict(files)
        self.bindings = dict(bindings)
        self._known = frozenset(files)

    def find_spec(self, fullname, path=None, target=None):
        if fullname in self.files:
            return _ztw_ilu.spec_from_loader(
                fullname,
                _ZtwPlayerLoader(self, fullname),
                origin='<player>/' + fullname + '.py')
        return None  # 非玩家名（或玩家模块的不受支持子模块）：交后续 finder

    def entry_is_async(self, fn):
        return _ztw_inspect.iscoroutinefunction(fn)


_ztw_registry = _ZtwPlayerRegistry()

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

_ztw_sys.meta_path = [_ztw_registry, _ZtwFinder()] + _ztw_sys.meta_path

# sys.path 只留发行物内的标准库目录（去 cwd 与 site-packages：不提供
# pip）。Windows 的 sys.path 条目可能以相对形式或与 prefix 不同大小写/
# 分隔符出现（盘符大小写、隔离式相对条目）——统一 normcase+normpath
# 后基于 prefix 解析判定并输出绝对条目；裸 startswith 会整条误杀
#（Windows CI 实测：import bisect 报 ModuleNotFoundError）。macOS 条目
# 本就是绝对路径，行为不变。os 在启动快照内，sys.modules 命中不走 finder。
import os as _ztw_os
_prefix = _ztw_sys.prefix
_ztw_prefix_key = _ztw_os.path.normcase(_ztw_os.path.normpath(_prefix))
_ztw_new_path = []
for _p in _ztw_sys.path:
    if not _p:
        continue  # '' 即 cwd：不提供按 cwd 导入
    _cand = _p if _ztw_os.path.isabs(_p) else _ztw_os.path.join(_prefix, _p)
    _key = _ztw_os.path.normcase(_ztw_os.path.normpath(_cand))
    if 'site-packages' in _key:
        continue
    if _key == _ztw_prefix_key or _key.startswith(_ztw_prefix_key + _ztw_os.sep):
        _ztw_new_path.append(_cand)
_ztw_sys.path = _ztw_new_path

# 前缀比较用规范化形态（两种分隔符都折成 '/'，Windows 文件系统大小写
# 不敏感故统一小写）：裸 startswith 可被 ../ 遍历与同级目录前缀碰撞
# 绕过。符号链接级逃逸超出误用防护定位。
_ztw_prefix_norm = '/'.join(
    s for s in _prefix.replace('\\', '/').split('/') if s not in ('', '.')
).lower()

def _ztw_under_prefix(p):
    segs = []
    for seg in p.replace('\\', '/').lower().split('/'):
        if seg in ('', '.'):
            continue
        if seg == '..':
            return False  # 任何上翻分量直接拒绝
        segs.append(seg)
    norm = '/'.join(segs)
    return norm == _ztw_prefix_norm or norm.startswith(_ztw_prefix_norm + '/')

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
    if event in ('open', 'open_code'):
        # 只放行标准库目录内的只读打开（import 系统读源码走 open_code）：
        # - 路径规范化后必须仍位于发行物前缀之下——裸 startswith 可被
        #   ../ 遍历与同级目录前缀碰撞绕过；
        # - 写模式一律拒绝——_io 保留在 sys.modules（import 机器的惰性
        #   依赖）后这是仅剩的文件写入面。mode 含 w/a/x/+ 同拒；flags
        #   低两位为访问模式（0 只读 / 1 写 / 2 读写，POSIX 与 Windows
        #   CRT 同值）。args = (path, mode, flags)。
        p = args[0] if args else None
        p_str = p.decode('utf-8', 'replace') if isinstance(p, bytes) else p
        mode = args[1] if len(args) > 1 and isinstance(args[1], str) else ''
        flags = args[2] if len(args) > 2 else None
        writable = any(c in mode for c in 'wax+') or (
            isinstance(flags, int) and flags & 3 != 0)
        if writable or not (isinstance(p, str) and _ztw_under_prefix(p)):
            raise RuntimeError(
                f"能力收窄：拒绝打开 {str(p_str)[:120]!r}（文件访问不可用，"
                "仅标准库目录内部只读；误用防护，非安全边界）")

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
# 模块内部的引用不受影响，只有"按名再导入"会走 finder 被拒。
# 下划线私有模块不再整类保留（评审发现：import builtins / _thread / _io
# 可直接取回危险对象、import __main__ 可取回真 builtins 字典、import
# _imp 的 create_dynamic 可无审计加载原生库），改为逐项实证的显式最小
# 保留（见 _ZTW_KEEP_PRIVATE）。_signal 一并移除：C 级 SIGINT 处理器在
# 引导期已注册且不依赖 sys.modules 条目，玩家再 import _signal 只会被
# finder 拒绝，无法用 SIG_IGN 拆除看门狗注入（第二层中断保真）。
_ZTW_PROTECTED = _ZTW_WHITELIST | {'builtins', '__ztw'}
# 显式最小保留，逐项实证依据（钉版 3.13.15 逐项踢出后全量探测）：
# - _io：import 机器的硬需求——_bootstrap_external.get_data 读源码时
#   惰性 import _io（sys.modules 命中）；其文件面由审计钩子封住。
# - _warnings：C 层告警路径 PyErr_WarnExplicitEx 经 PyImport_ImportModule
#   ("_warnings") 取模块，被 finder 拒绝会把告警变成错误。
# frozen importlib 链 / _imp / _codecs 均不依赖 sys.modules 条目（import
# 机器持有模块对象引用），且保留会开放危险面——尤其 `import _imp` 的
# create_dynamic 可加载任意原生库且不经过任何审计事件。
_ZTW_KEEP_PRIVATE = frozenset(__ZTW_KEEP_PRIVATE__)
for _m in [m for m in list(_ztw_sys.modules)
           if m.split('.', 1)[0] not in _ZTW_PROTECTED
           and m.split('.', 1)[0] not in _ZTW_KEEP_PRIVATE]:
    del _ztw_sys.modules[_m]

# 玩家模块的 __spec__/__loader__/__file__ 属性链可达注册表与 loader
# 对象，进而经 find_spec.__globals__ 到达收窄命名空间（见上方预导入
# 注释）——与下述属性链残余同类，一并按误用防护口径接受。
# 已加载模块的属性链（collections._sys、json.decoder.re 等）仍指向真实
# 模块对象——逐链追捕不可靠，改为在源头拆除危险入口：
# - 真 builtins 模块删除 open/input/print/breakpoint（玩家白名单 dict
#   早已不含它们，这里断的是"绕回真 builtins"的链——print 污染 stdout
#   即 IPC 出流、input 读 stdin 即 IPC 入流、open 是文件面）；exec/eval/
#   compile 保留：collections.namedtuple 运行期依赖 eval，且沙箱内它们
#   只能做纯计算。宿主与绑定层自身不使用被拆除的入口。
# - sys 拆除 stdin/stdout（含 __stdin__/__stdout__ 备份）：stdin/stdout
#   是 IPC 帧流，Python 层写入/读取都会破坏协议；stderr 留给诊断。
for _n in ('open', 'input', 'print', 'breakpoint'):
    if hasattr(_b, _n):
        delattr(_b, _n)
for _n in ('stdin', 'stdout', '__stdin__', '__stdout__'):
    if hasattr(_ztw_sys, _n):
        delattr(_ztw_sys, _n)
"#;

/// 执行收窄。返回 `(_ztw_new_builtins, _ztw_player_registry)` 两个可调用
/// /对象（每次建立玩家命名空间时调用前者产出白名单 __builtins__ dict；
/// 每次 init 调后者的 update 更新玩家文件集与绑定注入）。
pub fn narrow(
    py: pyo3::Python<'_>,
) -> pyo3::PyResult<(pyo3::Py<pyo3::PyAny>, pyo3::Py<pyo3::PyAny>)> {
    use pyo3::types::PyDictMethods;
    let code = NARROW_PY
        .replace("__ZTW_WHITELIST__", &format!("{:?}", WHITELIST))
        .replace("__ZTW_TRANSITIVE__", &format!("{:?}", TRANSITIVE))
        .replace("__ZTW_KEEP_PRIVATE__", &format!("{:?}", KEEP_PRIVATE));
    let globals = pyo3::types::PyDict::new(py);
    py.run(
        &std::ffi::CString::new(code).expect("收窄源码无 NUL"),
        Some(&globals),
        None,
    )?;
    let get = |key: &'static str| {
        globals
            .get_item(key)?
            .map(pyo3::Py::from)
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(key))
    };
    Ok((get("_ztw_new_builtins")?, get("_ztw_registry")?))
}
