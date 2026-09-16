# 宿主侧 Game 绑定（A1 Python 版）。
# 本文件是绑定层的唯一实现，由 ztw-api 内嵌（BOOTSTRAP_PY）、宿主在每次
# 执行环境重建后执行进玩家命名空间；不是玩家代码。语义逐项对照
# bindings/bootstrap.js——两处改动的任何行为差异都必须同步（人工同步点
# 由 a1 双语言矩阵测试锚定）。
#
# 协议约定（与 bootstrap.js 相同）：
#  - __ztw.__ipc(op, payloadJson) -> resultJson：同步 IPC 往返（Rust 注册）。
#    结果统一 {"ok":true,...} / {"ok":false,"code","message"}，!ok 抛带
#    code 属性的 GameError。
#  - _ztw_set_mirror(json, gen)：每次执行前由宿主注入全量镜像与 memory 代次。
#  - 管理操作响应携带镜像增量，按同一 FIFO 回放；回放失败整体失效，
#    下一次查询经 mirror.fetch 重建（docs/architecture/03）。
#  - memory 线值为外部标记枚举：
#    "Null" | {"Bool":b} | {"Num":n} | {"Str":s} | {"List":[..]} | {"Map":[[k,v],..]}

import json as _json
import math as _math
import types as _types
from collections.abc import MutableMapping, MutableSequence

import __ztw  # 原生桥（宿主注册于 sys.modules；sys.modules 命中绕过白名单 finder）

# ---- 结果码表（与 ztw-model codes::ALL 一致） -----------------------------

E = {
    "OK": "OK", "ARRIVED": "ARRIVED", "ALREADY_ACTED": "ALREADY_ACTED",
    "INVALID_ARGUMENT": "INVALID_ARGUMENT", "NOT_ADJACENT": "NOT_ADJACENT",
    "LOADED": "LOADED", "NOT_CARRYING": "NOT_CARRYING",
    "NOT_ENOUGH_ENERGY": "NOT_ENOUGH_ENERGY", "TARGET_FULL": "TARGET_FULL",
    "BOX_NOT_FOUND": "BOX_NOT_FOUND", "WRONG_GOODS": "WRONG_GOODS",
    "INVALID_TARGET": "INVALID_TARGET", "OUT_OF_BOUNDS": "OUT_OF_BOUNDS",
    "CELL_BLOCKED": "CELL_BLOCKED", "CELL_OCCUPIED": "CELL_OCCUPIED",
    "NO_SUCH_OBJECT": "NO_SUCH_OBJECT", "NO_PATH": "NO_PATH",
    "CELL_CONTESTED": "CELL_CONTESTED", "TARGET_CONTESTED": "TARGET_CONTESTED",
    "CHAIN_BLOCKED": "CHAIN_BLOCKED", "CHARGER_BUSY": "CHARGER_BUSY",
    "TARGET_MOVED": "TARGET_MOVED", "TARGET_GONE": "TARGET_GONE",
    "NO_FUNDS": "NO_FUNDS", "CREDIT_EXCEEDED": "CREDIT_EXCEEDED",
    "ON_VEHICLE": "ON_VEHICLE", "NO_FREE_DOCK": "NO_FREE_DOCK",
    "NOT_EMPTY": "NOT_EMPTY", "HAS_VEHICLE": "HAS_VEHICLE",
    "ORDER_GONE": "ORDER_GONE", "GOODS_MOVED": "GOODS_MOVED",
    "NOT_ON_WALL": "NOT_ON_WALL", "INIT_PHASE": "INIT_PHASE",
}


class GameError(Exception):
    """变更型调用失败（ok:false）：code 为结果码，message 为人读说明。"""

    def __init__(self, code, message):
        super().__init__(message or code)
        self.code = code


def _ipc(op, obj=None):
    payload = "{}" if obj is None else _json.dumps(obj)
    res = _json.loads(__ztw.__ipc(op, payload))
    if isinstance(res, dict) and res.get("ok") is False:
        raise GameError(res.get("code", "UNKNOWN"), op + ": " + res.get("message", ""))
    return res


# ---- Position 值对象（具名元组：解构/下标/等值/可作字典键） ----------------

class Position(tuple):
    __slots__ = ()

    def __new__(cls, x, y):
        return super().__new__(cls, (int(x), int(y)))

    @property
    def x(self):
        return self[0]

    @property
    def y(self):
        return self[1]

    @classmethod
    def of(cls, o):
        if isinstance(o, Position):
            return o
        if isinstance(o, (list, tuple)) and len(o) == 2:
            return cls(o[0], o[1])
        x = getattr(o, "x", None)
        y = getattr(o, "y", None)
        if isinstance(x, (int, float)) and isinstance(y, (int, float)):
            return cls(x, y)
        return None

    def equals(self, other):
        p = Position.of(other)
        return p is not None and self[0] == p[0] and self[1] == p[1]

    def adjacent(self, other):
        p = Position.of(other)
        if p is None:
            return False
        return abs(self[0] - p[0]) + abs(self[1] - p[1]) == 1

    def distance(self, other):
        p = Position.of(other)
        if p is None:
            raise ValueError("distance 需要可解析的位置")
        return abs(self[0] - p[0]) + abs(self[1] - p[1])

    def neighbors(self):
        return [
            Position(self[0], self[1] - 1),
            Position(self[0] + 1, self[1]),
            Position(self[0], self[1] + 1),
            Position(self[0] - 1, self[1]),
        ]

    def __str__(self):
        return "(" + str(self[0]) + "," + str(self[1]) + ")"


# ---- 镜像状态 -----------------------------------------------------------

M = None      # 解析后的镜像
GEN = 0       # memory 会话代次
stale = False  # 增量回放失败 → 整体失效
_handle_cache = {}  # "gen:node" -> 包装器（同一节点同一包装器实例）
_DROP_DELTAS = False  # 故障注入钩子：置位后所有镜像增量被丢弃
_stats = {"delta_us": 0, "delta_count": 0, "rebuild_us": 0}


def _ztw_set_mirror(mirror_json, gen):
    global M, GEN, stale
    M = _json.loads(mirror_json)
    # 句柄缓存以代次为键：同会话代次内同一节点必须返回同一包装器实例
    # （docs/architecture/06 句柄驻留与相等性），只在代次变化时清空。
    if gen != GEN:
        _handle_cache.clear()
    GEN = gen
    stale = False


def _ztw_stats():
    return _stats


def _ztw_set_drop_deltas(flag):
    """故障注入钩子（宿主调用）：置位后所有镜像增量被丢弃。

    命名空间隔离后玩家 ns 里的同名变量不影响本模块全局，注入须经此
    setter（与 _ztw_set_mirror/_ztw_stats 同款导出方式）。
    """
    global _DROP_DELTAS
    _DROP_DELTAS = bool(flag)


def _ensure_mirror():
    global M, stale
    if M is not None and not stale:
        return
    t0 = __ztw.__now_us()
    r = _ipc("mirror.fetch", {})
    # v4：镜像以原始 JSON 值内嵌于回复，免一层 loads。
    M = r["mirror"]
    stale = False
    _stats["rebuild_us"] += __ztw.__now_us() - t0


def _apply_delta(d):
    global stale
    t0 = __ztw.__now_us()
    try:
        if M is None or stale or _DROP_DELTAS:
            stale = True  # 丢弃 / 失败不猜测，整体重建
            return
        if isinstance(d.get("gold_milli"), str):
            M["gold_milli"] = d["gold_milli"]
        if d.get("kind") == "take":
            gone = d.get("remove_listing")
            if isinstance(gone, int):
                M["sell_orders"] = [o for o in M.get("sell_orders", []) if o["id"] != gone]
                M["buy_orders"] = [o for o in M.get("buy_orders", []) if o["id"] != gone]
                if not any(o["id"] == d["add_my_order"]["id"] for o in M.get("my_orders", [])):
                    M.setdefault("my_orders", []).append(d["add_my_order"])
        elif d.get("kind") == "cancel":
            if isinstance(d.get("debt_milli"), str):
                M["debt_milli"] = d["debt_milli"]
            M["my_orders"] = [o for o in M.get("my_orders", []) if o["id"] != d["remove_my_order"]]
            if isinstance(d.get("remove_vehicle"), int):
                M["vehicles"] = [v for v in M.get("vehicles", []) if v["id"] != d["remove_vehicle"]]
            if d.get("update_dock"):
                for dock in M.get("docks", []):
                    if dock["id"] == d["update_dock"]["id"]:
                        dock["docked_vehicle"] = d["update_dock"]["docked_vehicle"]
        elif d.get("kind") == "destroy":
            if d.get("rebuild"):
                stale = True  # 嵌套视图受影响（如销毁被携带货物），整体重建
                return
            lists = {
                "robot": "robots", "shelf": "shelves", "charger": "chargers",
                "dock": "docks", "box": "ground_boxes",
            }
            key = lists.get(d.get("object"))
            if key is None:
                stale = True
                return
            M[key] = [o for o in M.get(key, []) or [] if o["id"] != d["id"]]
            if isinstance(d.get("unblock"), list) and d["unblock"]:
                drop = {(c[0], c[1]) for c in d["unblock"]}
                M["blocked"] = [c for c in M.get("blocked", []) if (c[0], c[1]) not in drop]
        elif d.get("kind") == "funds":
            # 借款 / 还款：只动金币与欠款（金币已在分支顶部统一回放）。
            if isinstance(d.get("debt_milli"), str):
                M["debt_milli"] = d["debt_milli"]
        elif d.get("kind") == "buy":
            # 新对象全量视图原样入列（与下一次全量镜像同源），blocked 追加。
            lists = {
                "robot": "robots", "shelf": "shelves",
                "charger": "chargers", "dock": "docks",
            }
            key = lists.get(d.get("object"))
            entry = d.get(d.get("object"))
            if key is None or not isinstance(entry, dict):
                stale = True  # 增量与对象类别不一致，不猜测
                return
            M.setdefault(key, []).append(entry)
            if isinstance(d.get("block"), list) and d["block"]:
                M["blocked"] = M.get("blocked", []) + d["block"]
        else:
            stale = True  # 未知增量种类不猜测
    except Exception:
        stale = True
    finally:
        _stats["delta_us"] += __ztw.__now_us() - t0
        _stats["delta_count"] += 1


# ---- 视图工厂 -----------------------------------------------------------

def _P(x, y):
    return Position(x, y)


def _box_view(b):
    return _types.SimpleNamespace(
        id=b["id"], goods_type=b["goods_type"], holder=b["holder"], location=_P(b["x"], b["y"])
    )


def _order_view(o):
    return _types.SimpleNamespace(
        id=o["id"],
        side=o["side"],
        goods_type=o["goods_type"],
        qty=o["qty"],
        # 经济值为十进制字符串传输（防 JSON 浮点精度），此处换算。
        unit_price=int(o["unit_price_milli"]) / 1000,
        vehicle=o.get("vehicle"),
        dock=o.get("dock"),
    )


def _id_of(x):
    if x is None:
        return None
    if isinstance(x, int) and not isinstance(x, bool):
        return x
    ident = getattr(x, "id", None)
    if isinstance(ident, int):
        return ident
    return None


def _num_ok(v):
    """坐标分量守卫：有限且 |v| < 2^31。NaN / Infinity 与超范围归一为
    非法参数（→ INVALID_ARGUMENT），不裸抛 ValueError/OverflowError——
    与 bootstrap.js 的 coordOk 同口径。"""
    return isinstance(v, (int, float)) and not isinstance(v, bool) \
        and _math.isfinite(v) and abs(v) < 2147483648


def _pos_of(x):
    """坐标参数宽进：Position / 两元素序列（含受控列表）/ {x,y} / 视图
    对象（机器人、货架、充电桩、装卸位取 pos，车辆取 interact_pos，地面
    货物取 location）——对象取坐标规则与 move_to 一致（docs/game-design/08）。
    """
    if isinstance(x, Position):
        return x
    try:
        if len(x) == 2:
            a, b = x[0], x[1]
            if _num_ok(a) and _num_ok(b):
                return _P(int(a), int(b))
    except (TypeError, ValueError, KeyError):
        pass
    for attr in ("pos", "interact_pos", "location"):
        p = getattr(x, attr, None)
        if isinstance(p, Position):
            return p
    if isinstance(x, dict):
        px, py = x.get("x"), x.get("y")
        if _num_ok(px) and _num_ok(py):
            return _P(int(px), int(py))
    return None


_MAX_SAFE = 9007199254740991


def _to_milli(v):
    """金额显示值 → milli（docs/game-design/08「金额为定点显示值」）。

    与 bootstrap.js 的 toMilli 位级一致：正数 half-up（floor(x*1000+0.5)），
    非有限数与超出安全整数范围拒绝（返回 None → INVALID_ARGUMENT）。
    0.1 这类不可精确表示的值两侧乘法误差相同，换算结果一致。
    """
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return None
    try:
        x = float(v) * 1000 + 0.5
    except OverflowError:
        # CPython 大 int 转 float 抛 OverflowError（JS 同输入得到 ±Infinity
        # 后走非有限拒绝）；两侧同样落到 None → INVALID_ARGUMENT。
        return None
    if not _math.isfinite(x):
        return None
    m = _math.floor(x)
    if abs(m) > _MAX_SAFE:
        return None
    return m


def _is_unit_step(p):
    dx, dy = p[0], p[1]
    if abs(dx) > 1 or abs(dy) > 1:
        return False
    return (dx != 0) != (dy != 0)


class _RobotView:
    __slots__ = ("id", "pos", "energy", "energy_max", "carry", "last_result")

    def __init__(self, r):
        self.id = r["id"]
        self.pos = _P(r["x"], r["y"])
        self.energy = r["energy"]
        self.energy_max = r["energy_max"]
        self.carry = _box_view(r["carry"]) if r.get("carry") else None
        lr = r.get("last_result")
        self.last_result = (
            _types.SimpleNamespace(action=lr["action"], arg=lr["arg"], code=lr["code"])
            if lr
            else None
        )

    @property
    def memory(self):
        res = _ipc("mem.robot_memory", {"gen": GEN, "robot_id": self.id})
        return _mem_proxy(res["node"], "map")

    def move(self, direction):
        p = Position.of(direction)
        if p is None or not _is_unit_step(p):
            return E["INVALID_ARGUMENT"]
        res = _ipc("robot.move", {"robot_id": self.id, "dx": p[0], "dy": p[1]})
        return res["code"]

    def charge(self):
        res = _ipc("robot.charge", {"robot_id": self.id})
        return res["code"]

    def take(self, target, box_id):
        t = _id_of(target)
        b = _id_of(box_id)
        if t is None or b is None:
            return E["INVALID_ARGUMENT"]
        res = _ipc("robot.take", {"robot_id": self.id, "target_id": t, "box_id": b})
        return res["code"]

    def give(self, target, box_id=None):
        t = _id_of(target)
        if t is None:
            return E["INVALID_ARGUMENT"]
        payload = {"robot_id": self.id, "target_id": t}
        b = _id_of(box_id)
        if b is not None:
            payload["box_id"] = b  # 缺省 = 当前携带物
        res = _ipc("robot.give", payload)
        return res["code"]

    def pick(self, x, y):
        res = _ipc("robot.pick", {"robot_id": self.id, "x": int(x), "y": int(y)})
        return res["code"]

    def drop(self, x, y, box_id=None):
        payload = {"robot_id": self.id, "x": int(x), "y": int(y)}
        b = _id_of(box_id)
        if b is not None:
            payload["box_id"] = b  # 缺省 = 当前携带物
        res = _ipc("robot.drop", payload)
        return res["code"]

    def move_to(self, target, second=None, opts=None):
        """复合移动：move_to(x, y, opts) 或 move_to(target, opts)——坐标
        解析本地完成（镜像在宿主），受理 / 缓存 / 寻路全在主进程单源实现
        （docs/architecture/04）。非法参数与 move 同款本地 INVALID_ARGUMENT；
        NaN / Infinity / 超 i32 范围归一拒绝（_num_ok，不裸抛）。
        """
        if isinstance(target, (int, float)) or isinstance(second, (int, float)):
            if not _num_ok(target) or not _num_ok(second):
                return E["INVALID_ARGUMENT"]
            goal, real_opts = _P(int(target), int(second)), opts
        else:
            goal, real_opts = _pos_of(target), second
        if goal is None:
            return E["INVALID_ARGUMENT"]
        payload = {"robot_id": self.id, "x": goal[0], "y": goal[1]}
        if isinstance(real_opts, dict) and "range" in real_opts:
            payload["range"] = real_opts["range"]  # 负值 / 错型由服务端拒绝
        res = _ipc("robot.move_to", payload)
        return res["code"]

    def __repr__(self):
        return f"<Robot #{self.id} pos={self.pos}>"


def _shelf_view(s):
    return _types.SimpleNamespace(
        id=s["id"], pos=_P(s["x"], s["y"]),
        boxes=[_box_view(b) for b in s["boxes"]], capacity=s["capacity"],
    )


def _charger_view(c):
    return _types.SimpleNamespace(id=c["id"], pos=_P(c["x"], c["y"]))


def _dock_view(d):
    return _types.SimpleNamespace(
        id=d["id"],
        pos=_P(d["x"], d["y"]),
        # 与 pos 同为 Position：支持 .x/.y/.equals 等（锚点 + ext 合成第二格）。
        ext=_P(d["ext"][0], d["ext"][1]),
        docked_vehicle=d.get("docked_vehicle"),
    )


def _vehicle_view(v):
    return _types.SimpleNamespace(
        id=v["id"],
        kind=v["kind"],
        goods_type=v["goods_type"],
        interact_pos=_P(v["x"], v["y"]),
        order_id=v.get("order_id"),
        dock=v.get("dock"),
        boxes=[_box_view(b) for b in v["boxes"]],
    )


# ---- 受控 memory 包装器 ----------------------------------------------------


def _to_wire(v, seen=None):
    """Python 值 → 线值。循环拒绝、原生容器深拷贝、Position→[x,y]，
    语义对照 bootstrap.js 的 toWire。"""
    if seen is None:
        seen = set()
    if v is None:
        return "Null"
    if isinstance(v, bool):  # bool 是 int 子类，先判
        return {"Bool": v}
    if isinstance(v, int):
        if abs(v) > _MAX_SAFE:
            raise GameError("INVALID_VALUE", "memory 拒绝超安全范围整数 " + str(v))
        return {"Num": v}
    if isinstance(v, float):
        if not _math.isfinite(v):
            raise GameError("INVALID_VALUE", "memory 拒绝非有限数字 " + str(v))
        return {"Num": v}
    if isinstance(v, str):
        return {"Str": v}
    if isinstance(v, Position):
        # 线值元素须逐个包装（与 bootstrap.js 同款修复，A1 矩阵对账锚点）。
        return {"List": [_to_wire(v[0], seen), _to_wire(v[1], seen)]}
    vid = id(v)
    if vid in seen:
        raise GameError("INVALID_VALUE", "memory 拒绝循环引用")
    seen.add(vid)
    try:
        if isinstance(v, (list, tuple)):
            return {"List": [_to_wire(x, seen) for x in v]}  # tuple → 列表（docs 06）
        if isinstance(v, dict):
            return {"Map": [[str(k), _to_wire(val, seen)] for k, val in v.items()]}
        if callable(getattr(v, "to_dict", None)):
            return _to_wire(v.to_dict(), seen)
        if callable(getattr(v, "to_list", None)):
            return _to_wire(v.to_list(), seen)
        raise GameError("INVALID_VALUE", "memory 不支持类型 " + type(v).__name__)
    finally:
        seen.discard(vid)


def _wire_to_plain(w):
    if w == "Null" or w is None:
        return None
    if isinstance(w, dict):
        if "Bool" in w:
            return w["Bool"]
        if "Num" in w:
            return w["Num"]
        if "Str" in w:
            return w["Str"]
        if "List" in w:
            return [_wire_to_plain(x) for x in w["List"]]
        if "Map" in w:
            return {k: _wire_to_plain(val) for k, val in w["Map"]}
    return None


def _read_result(r):
    if r["t"] == "scalar":
        return r["v"]
    if r["t"] == "handle":
        return _mem_proxy(r["node"], r["kind"])
    raise GameError("INVALID_VALUE", "memory 读取结果异常")


def _mem_proxy(node, kind):
    key = str(GEN) + ":" + str(node)
    hit = _handle_cache.get(key)
    if hit is not None:
        return hit
    proxy = MemoryMap(GEN, node) if kind == "map" else MemoryList(GEN, node)
    _handle_cache[key] = proxy
    return proxy


class MemoryMap(MutableMapping):
    """受控映射：标量读写是一次同步 IPC（与 JS 版一致）；容器键查找经
    槽位缓存（key → 子代理，写透失效，代次变化随 _handle_cache 重建）。
    键一律转字符串键；相等性按（会话代次, 节点号）判定（docs 06）。
    操作面以文档为准——变更型 ABC 混入（pop/update/clear 等）显式拒绝。"""

    def __init__(self, gen, node):
        self._gen = gen
        self._node = node
        # 槽位缓存：key → 子代理实例（只缓存 handle 型结果）。命中还需
        # _gen == GEN（跨代一律走 IPC，由服务端 check_gen 拒绝）。
        self._slots = {}
        # 毒化标志：父级替换 / 删除本槽位时置位，后续访问本地抛 STALE
        # （与服务端 kill_subtree 的 live() 检查同码）。
        self._dead = False

    def _raise_if_dead(self):
        if self._dead:
            raise GameError("STALE_MEMORY_REFERENCE", "容器已失效")

    @staticmethod
    def _poison_tree(child):
        """递归毒化缓存子树：标记死亡并连带其槽位内的全部后代代理。"""
        child._dead = True
        for grand in list(child._slots.values()):
            MemoryMap._poison_tree(grand)
        child._slots.clear()

    def _invalidate(self, k):
        child = self._slots.pop(k, None)
        if child is not None:
            self._poison_tree(child)

    def __eq__(self, other):
        return (
            isinstance(other, MemoryMap)
            and other._gen == self._gen
            and other._node == self._node
        )

    # docs 06：支持的操作由文档明确列出，不承诺全部原生容器方法。ABC
    # 混入的变更型方法会绕过文档面"意外可用"（今天能用、文档没写、
    # 绑定层一重构即断），显式拒绝并给可读指引；只读协议方法
    #（get/keys/items/values/in）是容器协议的自然组成，保留。
    def pop(self, *_args):
        raise GameError("INVALID_OPERATION", "受控映射不支持 pop（删除请用 del m[key]）")

    def popitem(self):
        raise GameError("INVALID_OPERATION", "受控映射不支持 popitem（删除请用 del m[key]）")

    def clear(self):
        raise GameError("INVALID_OPERATION", "受控映射不支持 clear（逐键删除请用 del m[key]）")

    def update(self, *_args, **_kwargs):
        raise GameError("INVALID_OPERATION", "受控映射不支持 update（写入请用 m[key] = value）")

    def setdefault(self, *_args):
        raise GameError("INVALID_OPERATION", "受控映射不支持 setdefault（读取请用 m[key] 或 in）")

    def __getitem__(self, key):
        self._raise_if_dead()
        k = str(key)
        if self._gen == GEN:
            hit = self._slots.get(k)
            if hit is not None:
                return hit
        r = _ipc("mem.map_get", {"gen": self._gen, "node": self._node, "key": k})
        if r["t"] == "handle":
            w = _mem_proxy(r["node"], r["kind"])
            self._slots[k] = w
            return w
        if r["t"] == "missing":
            raise KeyError(key)
        return r["v"]

    def __setitem__(self, key, value):
        self._raise_if_dead()
        k = str(key)
        _ipc("mem.map_set", {
            "gen": self._gen, "node": self._node, "key": k, "value": _to_wire(value),
        })
        self._invalidate(k)

    def __delitem__(self, key):
        self._raise_if_dead()
        k = str(key)
        _ipc("mem.map_delete", {"gen": self._gen, "node": self._node, "key": k})
        self._invalidate(k)

    def __iter__(self):
        self._raise_if_dead()
        return iter(_ipc("mem.map_keys", {"gen": self._gen, "node": self._node})["keys"])

    def __len__(self):
        self._raise_if_dead()
        return _ipc("mem.map_size", {"gen": self._gen, "node": self._node})["value"]

    def to_dict(self):
        self._raise_if_dead()
        return _wire_to_plain(_ipc("mem.to_value", {"gen": self._gen, "node": self._node})["value"])

    def __repr__(self):
        return f"<Game.memory map node#{self._node}>"


class MemoryList(MutableSequence):
    """受控序列：下标读写、append/remove(i)；删除元素用 remove(index)
    （按**下标**删除——与 collections.abc.Sequence.remove 按值删除不同），
    del / insert 不支持。"""

    def __init__(self, gen, node):
        self._gen = gen
        self._node = node
        # 与 MemoryMap._slots 同构：下标 → 子代理；set(i) 毒化旧子代理，
        # remove 使下标整体平移但节点存活——只全清不毒化。
        self._slots = {}
        self._dead = False

    def __eq__(self, other):
        return (
            isinstance(other, MemoryList)
            and other._gen == self._gen
            and other._node == self._node
        )

    def _raise_if_dead(self):
        if self._dead:
            raise GameError("STALE_MEMORY_REFERENCE", "容器已失效")

    def _check_index(self, i):
        if not isinstance(i, int) or isinstance(i, bool) or i < 0:
            raise GameError("INVALID_ARGUMENT", "下标需要非负整数")

    def __getitem__(self, index):
        self._raise_if_dead()
        self._check_index(index)
        if self._gen == GEN:
            hit = self._slots.get(index)
            if hit is not None:
                return hit
        r = _ipc("mem.list_get", {"gen": self._gen, "node": self._node, "index": index})
        if r["t"] == "handle":
            w = _mem_proxy(r["node"], r["kind"])
            self._slots[index] = w
            return w
        if r["t"] == "missing":
            raise IndexError(index)
        return r["v"]

    def __setitem__(self, index, value):
        self._raise_if_dead()
        self._check_index(index)
        _ipc("mem.list_set", {
            "gen": self._gen, "node": self._node, "index": index, "value": _to_wire(value),
        })
        child = self._slots.pop(index, None)
        if child is not None:
            MemoryMap._poison_tree(child)

    def __delitem__(self, index):
        raise GameError("INVALID_OPERATION", "受控列表删除请使用 remove(index)")

    def __iter__(self):
        self._raise_if_dead()
        entries = _ipc("mem.list_entries", {"gen": self._gen, "node": self._node})["entries"]
        for r in entries:
            yield _read_result(r)

    def __len__(self):
        self._raise_if_dead()
        return _ipc("mem.list_size", {"gen": self._gen, "node": self._node})["value"]

    def append(self, value):
        self._raise_if_dead()
        _ipc("mem.list_append", {"gen": self._gen, "node": self._node, "value": _to_wire(value)})

    def remove(self, index):
        """按下标删除（与 JS 版 remove(i) 一致）。"""
        self._raise_if_dead()
        self._check_index(index)
        _ipc("mem.list_remove", {"gen": self._gen, "node": self._node, "index": index})
        # 被删下标的节点死亡（毒化）；其余节点平移存活，只清缓存。
        child = self._slots.pop(index, None)
        if child is not None:
            MemoryMap._poison_tree(child)
        self._slots.clear()

    def insert(self, index, value):
        raise GameError("INVALID_OPERATION", "受控列表不支持 insert（用 append/remove 组合）")

    def pop(self, *_args):
        raise GameError("INVALID_OPERATION", "受控列表不支持 pop（删除请用 remove(index)）")

    def extend(self, *_args):
        raise GameError("INVALID_OPERATION", "受控列表不支持 extend（追加请逐个 append）")

    def clear(self):
        # 显式覆写：MutableSequence.clear 的混入实现是 while True: self.pop()
        # ——不覆写则靠 pop 的拒绝间接拦截，报错文案指向 pop（指鹿为马），
        # 且 pop 覆写一旦调整 clear 会静默变可用。
        raise GameError("INVALID_OPERATION", "受控列表不支持 clear（删除请用 remove(index)）")

    def to_list(self):
        self._raise_if_dead()
        return _wire_to_plain(_ipc("mem.to_value", {"gen": self._gen, "node": self._node})["value"])

    def __repr__(self):
        return f"<Game.memory list node#{self._node}>"


# ---- Game 门面 ------------------------------------------------------------

class _Market:
    def sell_orders(self):
        _ensure_mirror()
        return [_order_view(o) for o in M.get("sell_orders", [])]

    def buy_orders(self):
        _ensure_mirror()
        return [_order_view(o) for o in M.get("buy_orders", [])]

    def take(self, order_id):
        res = _ipc("market.take", {"order_id": _id_of(order_id)})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]

    def cancel(self, order_id):
        res = _ipc("market.cancel", {"order_id": _id_of(order_id)})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]


class Game:
    E = E
    NORTH = Position(0, -1)
    SOUTH = Position(0, 1)
    WEST = Position(-1, 0)
    EAST = Position(1, 0)

    market = _Market()

    def robots(self):
        _ensure_mirror()
        return [_RobotView(r) for r in M.get("robots", [])]

    def shelves(self):
        _ensure_mirror()
        return [_shelf_view(s) for s in M.get("shelves", [])]

    def chargers(self):
        _ensure_mirror()
        return [_charger_view(c) for c in M.get("chargers", [])]

    def docks(self):
        _ensure_mirror()
        return [_dock_view(d) for d in M.get("docks", [])]

    def vehicles(self, kind=None):
        _ensure_mirror()
        vs = [_vehicle_view(v) for v in M.get("vehicles", [])]
        return vs if kind is None else [v for v in vs if v.kind == kind]

    def ground_boxes(self):
        _ensure_mirror()
        return [_box_view(b) for b in M.get("ground_boxes", [])]

    def my_orders(self):
        _ensure_mirror()
        return [_order_view(o) for o in M.get("my_orders", [])]

    def objects_at(self, x, y):
        _ensure_mirror()
        found = []
        for key, view in (
            ("robots", _RobotView), ("shelves", _shelf_view), ("chargers", _charger_view),
            ("docks", _dock_view), ("vehicles", _vehicle_view), ("ground_boxes", _box_view),
        ):
            for raw in M.get(key, []) or []:
                if raw["x"] == x and raw["y"] == y:
                    found.append((raw["id"], view(raw)))
        found.sort(key=lambda e: e[0])
        return [e[1] for e in found]

    def get_object_by_id(self, object_id):
        _ensure_mirror()
        object_id = _id_of(object_id)
        for key, view in (
            ("robots", _RobotView), ("shelves", _shelf_view), ("chargers", _charger_view),
            ("docks", _dock_view), ("vehicles", _vehicle_view), ("ground_boxes", _box_view),
        ):
            for raw in M.get(key, []) or []:
                if raw["id"] == object_id:
                    return view(raw)
        for key in ("my_orders", "sell_orders", "buy_orders"):
            for o in M.get(key, []) or []:
                if o["id"] == object_id:
                    return _order_view(o)
        return None

    def map_size(self):
        _ensure_mirror()
        return (M["map_w"], M["map_h"])

    def find_path(self, start, goal, opts=None):
        """静态障碍最短路径（docs/game-design/08「寻路」）：忠实三分支——
        路径 / []（已在到达范围）/ None（不可达），不遮蔽空列表陷阱。
        计算与配额在主进程（docs/architecture/03：find_path 不进镜像）。
        """
        s, g = _pos_of(start), _pos_of(goal)
        if s is None or g is None:
            raise GameError("INVALID_ARGUMENT",
                            "find_path: start/goal 须为坐标或对象")
        payload = {"sx": s[0], "sy": s[1], "gx": g[0], "gy": g[1]}
        if isinstance(opts, dict) and "range" in opts:
            payload["range"] = opts["range"]  # 负值 / 错型由服务端拒绝
        res = _ipc("game.find_path", payload)
        if res["path"] is None:
            return None
        return [_P(q[0], q[1]) for q in res["path"]]

    @property
    def gold(self):
        _ensure_mirror()
        return int(M["gold_milli"]) / 1000

    @property
    def debt(self):
        _ensure_mirror()
        return int(M["debt_milli"]) / 1000

    @property
    def tick(self):
        _ensure_mirror()
        return M["tick"]

    def buy(self, kind, x, y):
        """购买设备（即时建成）。装卸位朝向由锚点边界墙唯一推导
        （角格 / 非墙格 NOT_ON_WALL）。"""
        res = _ipc("manage.buy", {"kind": str(kind), "x": int(x), "y": int(y)})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]

    def borrow(self, amount):
        m = _to_milli(amount)
        if m is None:
            return E["INVALID_ARGUMENT"]
        res = _ipc("manage.borrow", {"amount_milli": m})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]

    def repay(self, amount):
        m = _to_milli(amount)
        if m is None:
            return E["INVALID_ARGUMENT"]
        res = _ipc("manage.repay", {"amount_milli": m})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]

    def destroy(self, target):
        res = _ipc("manage.destroy", {"target_id": _id_of(target)})
        if res.get("delta"):
            _apply_delta(res["delta"])
        return res["code"]

    def log(self, *args):
        line = " ".join(a if isinstance(a, str) else _json.dumps(a, default=str) for a in args)
        _ipc("log", {"line": line})

    @property
    def memory(self):
        return _mem_proxy(0, "map")


Game = Game()
