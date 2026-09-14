# 能力收窄加固探针（评审 P2）：sys.modules 最小保留后，绕行链上的
# 危险入口必须已在源头拆除。只验证误用防护，不宣称恶意代码安全。
import builtins
import collections
import json as _json

_s = collections._sys  # 已知残余链（文档化）：真 sys 模块对象仍可达
_lib_file = _json.__file__  # 发行物前缀内的真实路径（只读面应放行）


def loop():
    for name in ("_signal", "_thread", "__main__", "_imp"):
        try:
            __import__(name)
            Game.log("import " + name + ": ALLOWED")
        except ImportError:
            Game.log("import " + name + ": DENIED")
        except Exception as e:  # noqa: BLE001  探针需要捕获一切
            Game.log("import " + name + ": " + type(e).__name__)
    # _io 因 import 机器惰性依赖保留，文件面由审计钩子封住。
    import _io
    Game.log("import _io: KEPT")
    for label, path, mode in [
        ("_io.read_outside", "/etc/hostname", "r"),
        ("_io.read_traversal", _lib_file + "/../../../../../etc/hostname", "r"),
        ("_io.write_inside", _lib_file + ".probe", "w"),
    ]:
        try:
            _io.open(path, mode)
            Game.log(label + ": ALLOWED")
        except RuntimeError:
            Game.log(label + ": RuntimeError")
        except Exception as e:  # noqa: BLE001  探针需要捕获一切
            Game.log(label + ": " + type(e).__name__)
    # 真 builtins：危险入口拆除（eval 保留——namedtuple 运行期依赖）。
    Game.log("builtins.open: " + ("GONE" if not hasattr(builtins, "open") else "PRESENT"))
    Game.log("builtins.print: " + ("GONE" if not hasattr(builtins, "print") else "PRESENT"))
    Game.log("builtins.input: " + ("GONE" if not hasattr(builtins, "input") else "PRESENT"))
    Game.log("builtins.eval: " + ("KEPT" if hasattr(builtins, "eval") else "GONE"))
    # IPC 帧流不可经 Python 层标准流触碰（残余链上的 sys 也一样）。
    Game.log("sys.stdout: " + ("GONE" if not hasattr(_s, "stdout") else "PRESENT"))
    Game.log("sys.stdin: " + ("GONE" if not hasattr(_s, "stdin") else "PRESENT"))
    Game.log("sys.stderr: KEPT" if hasattr(_s, "stderr") else "sys.stderr: GONE")
    # 只读面属文档化残余（误用防护定位）：sys.modules 仍可见。
    Game.log("sys.modules: " + ("PRESENT" if hasattr(_s, "modules") else "GONE"))
