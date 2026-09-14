# 能力收窄探针：白名单外 import 与 open 必须给可读错误（docs 08 A1
# 验收「能力收窄」行：仅验证误用防护，不宣称恶意代码安全）。
def loop():
    for name in ("os", "itertools", "re", "random", "time", "sys", "subprocess", "socket"):
        try:
            __import__(name)
            Game.log("import " + name + ": ALLOWED")
        except ImportError:
            Game.log("import " + name + ": DENIED")
        except Exception as e:  # noqa: BLE001  探针需要捕获一切
            Game.log("import " + name + ": " + type(e).__name__)
    try:
        open("/etc/hostname")
        Game.log("open: ALLOWED")
    except NameError:
        Game.log("open: DENIED")
