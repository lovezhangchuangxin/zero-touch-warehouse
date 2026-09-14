# 白名单内模块及其传递依赖全部可用（纯计算集）。
import bisect
import cmath
import collections
import decimal
import functools
import heapq
import json
import math
import numbers
import types
from collections import OrderedDict, defaultdict, deque, namedtuple


def loop():
    q = []
    heapq.heappush(q, (2, "a"))
    heapq.heappush(q, (1, "b"))
    Game.memory["heap_head"] = heapq.heappop(q)[0]
    Game.memory["facts"] = math.factorial(6)
    Game.memory["bisect"] = bisect.bisect_left([1, 3, 5], 3)
    pt = namedtuple("Pt", ["x", "y"])(3, 4)
    Game.memory["pt"] = [pt.x, pt.y]
    Game.memory["reduce"] = functools.reduce(lambda a, b: a + b, range(10))
    Game.log("json", json.dumps({"ok": True}))
    Game.log("decimal", str(decimal.Decimal("1.1") + decimal.Decimal("2.2")))
