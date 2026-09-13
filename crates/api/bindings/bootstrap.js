// 宿主侧 Game 绑定（A0 最小子集）。
// 本文件是绑定层的唯一实现，由 ztw-api 内嵌、runtime 在每次执行环境
// 重建后求值；不是玩家代码。
//
// 协议约定：
//  - __ipc(op, payloadJson) -> resultJson：同步 IPC 往返（Rust 注册）。
//    结果统一 {"ok":true,...} / {"ok":false,"code","message"}，!ok 抛带
//    code 属性的 Error。
//  - __setMirror(json, gen)：每次执行前由宿主注入全量镜像与 memory 代次。
//  - 管理操作响应携带镜像增量，按同一 FIFO 回放；回放失败整体失效，
//    下一次查询经 mirror.fetch 重建（docs/architecture/03）。
//  - memory 线值为外部标记枚举：
//    "Null" | {"Bool":b} | {"Num":n} | {"Str":s} | {"List":[..]} | {"Map":[[k,v],..]}
"use strict";

(function () {
  // ---- 结果码表（与 ztw-model codes::ALL 一致） ---------------------------
  const E = Object.freeze({
    OK: "OK", ARRIVED: "ARRIVED", ALREADY_ACTED: "ALREADY_ACTED",
    INVALID_ARGUMENT: "INVALID_ARGUMENT", NOT_ADJACENT: "NOT_ADJACENT",
    LOADED: "LOADED", NOT_CARRYING: "NOT_CARRYING",
    NOT_ENOUGH_ENERGY: "NOT_ENOUGH_ENERGY", TARGET_FULL: "TARGET_FULL",
    BOX_NOT_FOUND: "BOX_NOT_FOUND", WRONG_GOODS: "WRONG_GOODS",
    INVALID_TARGET: "INVALID_TARGET", OUT_OF_BOUNDS: "OUT_OF_BOUNDS",
    CELL_BLOCKED: "CELL_BLOCKED", CELL_OCCUPIED: "CELL_OCCUPIED",
    NO_SUCH_OBJECT: "NO_SUCH_OBJECT", NO_PATH: "NO_PATH",
    CELL_CONTESTED: "CELL_CONTESTED", TARGET_CONTESTED: "TARGET_CONTESTED",
    CHAIN_BLOCKED: "CHAIN_BLOCKED", CHARGER_BUSY: "CHARGER_BUSY",
    TARGET_MOVED: "TARGET_MOVED", TARGET_GONE: "TARGET_GONE",
    NO_FUNDS: "NO_FUNDS", CREDIT_EXCEEDED: "CREDIT_EXCEEDED",
    ON_VEHICLE: "ON_VEHICLE", NO_FREE_PORT: "NO_FREE_PORT",
    NOT_EMPTY: "NOT_EMPTY", HAS_VEHICLE: "HAS_VEHICLE",
    ORDER_GONE: "ORDER_GONE", GOODS_MOVED: "GOODS_MOVED",
    INIT_PHASE: "INIT_PHASE",
  });

  function ipcError(code, message) {
    const err = new Error(message || code);
    err.code = code;
    return err;
  }

  function rt(op, obj) {
    const payload = obj === undefined ? "{}" : JSON.stringify(obj);
    const res = JSON.parse(__ipc(op, payload));
    if (res && res.ok === false) {
      throw ipcError(res.code, op + ": " + (res.message || res.code));
    }
    return res;
  }
  function rtVoid(op, obj) {
    rt(op, obj);
  }

  // ---- Position 值对象（下标 + 展开 + equals/adjacent/distance/neighbors）--

  class Position extends Array {
    constructor(x, y) {
      super();
      this[0] = x | 0;
      this[1] = y | 0;
    }
    get x() { return this[0]; }
    get y() { return this[1]; }
    static of(o) {
      if (o instanceof Position) return o;
      if (Array.isArray(o)) return new Position(o[0], o[1]);
      if (o && typeof o.x === "number" && typeof o.y === "number") {
        return new Position(o.x, o.y);
      }
      return null;
    }
    equals(other) {
      const p = Position.of(other);
      return p !== null && this[0] === p[0] && this[1] === p[1];
    }
    adjacent(other) {
      const p = Position.of(other);
      if (p === null) return false;
      const dx = Math.abs(this[0] - p[0]);
      const dy = Math.abs(this[1] - p[1]);
      return dx + dy === 1;
    }
    distance(other) {
      const p = Position.of(other);
      if (p === null) return NaN;
      return Math.abs(this[0] - p[0]) + Math.abs(this[1] - p[1]);
    }
    neighbors() {
      return [
        new Position(this[0], this[1] - 1),
        new Position(this[0] + 1, this[1]),
        new Position(this[0], this[1] + 1),
        new Position(this[0] - 1, this[1]),
      ];
    }
    toString() { return "(" + this[0] + "," + this[1] + ")"; }
  }

  // ---- 镜像状态 -----------------------------------------------------------

  let M = null;      // 解析后的镜像
  let GEN = 0;       // memory 会话代次
  let stale = false; // 增量回放失败 → 整体失效
  const handleCache = new Map(); // "gen:node" -> 代理（同一节点同一包装器）

  globalThis.__setMirror = function (json, gen) {
    M = JSON.parse(json);
    // 句柄缓存以代次为键：同会话代次内同一节点必须返回同一包装器
    // 实例（docs/architecture/06 句柄驻留与相等性），因此只在代次变化
    // 时清空，跨 tick 不清。
    if (gen !== GEN) handleCache.clear();
    GEN = gen;
    stale = false;
  };
  // 故障注入钩子：置位后所有镜像增量被丢弃（模拟回放失败路径）。
  globalThis.__DROP_DELTAS = false;

  function ensureMirror() {
    if (M !== null && !stale) return false;
    const t0 = __nowUs();
    const r = rt("mirror.fetch", {});
    M = JSON.parse(r.mirror);
    stale = false;
    mirrorStats.rebuild_us += __nowUs() - t0;
    return true;
  }

  // 增量回放耗时累计（量测用；__nowUs 由宿主注册，µs 精度）。
  const mirrorStats = { delta_us: 0, delta_count: 0, rebuild_us: 0 };
  globalThis.__mirrorStats = function () { return mirrorStats; };

  function applyDelta(d) {
    const t0 = __nowUs();
    try {
      if (M === null || stale || globalThis.__DROP_DELTAS) {
        stale = true; // 丢弃 / 失败不猜测，整体重建
        return;
      }
      if (typeof d.gold_milli === "string") M.gold_milli = d.gold_milli;
      if (d.kind === "take") {
        const gone = d.remove_listing;
        if (typeof gone === "number") {
          M.sell_orders = (M.sell_orders || []).filter(function (o) { return o.id !== gone; });
          M.buy_orders = (M.buy_orders || []).filter(function (o) { return o.id !== gone; });
          const already = (M.my_orders || []).some(function (o) { return o.id === d.add_my_order.id; });
          if (!already) M.my_orders.push(d.add_my_order);
        }
      } else if (d.kind === "cancel") {
        if (typeof d.debt_milli === "string") M.debt_milli = d.debt_milli;
        M.my_orders = (M.my_orders || []).filter(function (o) { return o.id !== d.remove_my_order; });
        if (typeof d.remove_vehicle === "number") {
          M.vehicles = (M.vehicles || []).filter(function (v) { return v.id !== d.remove_vehicle; });
        }
        if (d.update_port) {
          const port = (M.ports || []).find(function (p) { return p.id === d.update_port.id; });
          if (port) port.docked_vehicle = d.update_port.docked_vehicle;
        }
      } else if (d.kind === "destroy") {
        if (d.rebuild) {
          stale = true; // 嵌套视图受影响（如销毁被携带货物），整体重建
          return;
        }
        const list = {
          robot: "robots", shelf: "shelves", charger: "chargers",
          port: "ports", box: "ground_boxes",
        }[d.object];
        if (!list) {
          stale = true;
          return;
        }
        M[list] = (M[list] || []).filter(function (o) { return o.id !== d.id; });
        if (Array.isArray(d.unblock) && d.unblock.length) {
          const drop = new Set(d.unblock.map(function (c) { return c[0] + "," + c[1]; }));
          M.blocked = (M.blocked || []).filter(function (c) { return !drop.has(c[0] + "," + c[1]); });
        }
      } else {
        stale = true; // 未知增量种类不猜测
      }
    } catch (e) {
      stale = true;
    } finally {
      mirrorStats.delta_us += __nowUs() - t0;
      mirrorStats.delta_count += 1;
    }
  }

  // ---- 视图工厂 ------------------------------------------------------------

  function P(x, y) { return new Position(x, y); }

  function boxView(b) {
    return { id: b.id, goods_type: b.goods_type, holder: b.holder, location: P(b.x, b.y) };
  }
  function orderView(o) {
    return {
      id: o.id,
      side: o.side,
      goods_type: o.goods_type,
      qty: o.qty,
      unit_price: Number(o.unit_price_milli) / 1000,
      vehicle: o.vehicle !== undefined ? o.vehicle : null,
      port: o.port !== undefined ? o.port : null,
    };
  }
  function robotView(r) {
    const view = {
      id: r.id,
      pos: P(r.x, r.y),
      energy: r.energy,
      energy_max: r.energy_max,
      carry: r.carry ? boxView(r.carry) : null,
      last_result: r.last_result
        ? { action: r.last_result.action, arg: r.last_result.arg, code: r.last_result.code }
        : null,
    };
    Object.defineProperty(view, "memory", {
      enumerable: false,
      get: function () {
        const res = rt("mem.robot_memory", { gen: GEN, robot_id: r.id });
        return memProxy(res.node, "map");
      },
    });
    view.move = function (dir) {
      const p = Position.of(dir);
      if (p === null || !isUnitStep(p)) return E.INVALID_ARGUMENT;
      const res = rt("robot.move", { robot_id: r.id, dx: p[0], dy: p[1] });
      return res.code;
    };
    view.charge = function () {
      const res = rt("robot.charge", { robot_id: r.id });
      return res.code;
    };
    view.take = function (target, boxId) {
      const t = idOf(target);
      const b = idOf(boxId);
      if (t === null || b === null) return E.INVALID_ARGUMENT;
      const res = rt("robot.take", { robot_id: r.id, target_id: t, box_id: b });
      return res.code;
    };
    view.give = function (target, boxId) {
      const t = idOf(target);
      if (t === null) return E.INVALID_ARGUMENT;
      const payload = { robot_id: r.id, target_id: t };
      const b = idOf(boxId);
      if (b !== null) payload.box_id = b; // 缺省 = 当前携带物
      const res = rt("robot.give", payload);
      return res.code;
    };
    view.pick = function (x, y) {
      const res = rt("robot.pick", { robot_id: r.id, x: x | 0, y: y | 0 });
      return res.code;
    };
    view.drop = function (x, y, boxId) {
      const payload = { robot_id: r.id, x: x | 0, y: y | 0 };
      const b = idOf(boxId);
      if (b !== null) payload.box_id = b; // 缺省 = 当前携带物
      const res = rt("robot.drop", payload);
      return res.code;
    };
    return view;
  }
  // 目标 / 货物参数接受对象或 id（docs/game-design/08 API 设计）。
  function idOf(x) {
    if (x === null || x === undefined) return null;
    if (typeof x === "number") return x;
    return typeof x.id === "number" ? x.id : null;
  }
  function isUnitStep(p) {
    const dx = p[0], dy = p[1];
    if (Math.abs(dx) > 1 || Math.abs(dy) > 1) return false;
    return (dx !== 0) !== (dy !== 0);
  }
  function shelfView(s) {
    return { id: s.id, pos: P(s.x, s.y), boxes: s.boxes.map(boxView), capacity: s.capacity };
  }
  function chargerView(c) {
    return { id: c.id, pos: P(c.x, c.y) };
  }
  function portView(p) {
    return { id: p.id, pos: P(p.x, p.y), docked_vehicle: p.docked_vehicle };
  }
  function vehicleView(v) {
    return {
      id: v.id,
      kind: v.kind,
      goods_type: v.goods_type,
      interact_pos: P(v.x, v.y),
      order_id: v.order_id,
      boxes: v.boxes.map(boxView),
    };
  }

  // ---- 受控 memory 代理 ----------------------------------------------------

  const MAX_SAFE = 9007199254740991;

  function toWire(v, seen) {
    seen = seen || new Set();
    if (v === undefined) {
      throw ipcError("INVALID_VALUE", "memory 不支持 undefined");
    }
    if (v === null) return "Null";
    switch (typeof v) {
      case "boolean": return { Bool: v };
      case "number":
        if (!isFinite(v)) throw ipcError("INVALID_VALUE", "memory 拒绝非有限数字 " + v);
        if (Number.isInteger(v) && Math.abs(v) > MAX_SAFE) {
          throw ipcError("INVALID_VALUE", "memory 拒绝超安全范围整数 " + v);
        }
        return { Num: v };
      case "string": return { Str: v };
      case "object": break;
      default:
        throw ipcError("INVALID_VALUE", "memory 不支持类型 " + typeof v);
    }
    if (seen.has(v)) throw ipcError("INVALID_VALUE", "memory 拒绝循环引用");
    seen.add(v);
    try {
      if (v instanceof Position) return { List: [v[0], v[1]] };
      if (Array.isArray(v)) {
        const items = [];
        for (let i = 0; i < v.length; i++) {
          if (!(i in v)) throw ipcError("INVALID_VALUE", "memory 拒绝稀疏数组");
          items.push(toWire(v[i], seen));
        }
        return { List: items };
      }
      if (typeof v.to_dict === "function") return toWire(v.to_dict(), seen);
      if (typeof v.to_list === "function") return toWire(v.to_list(), seen);
      const pairs = [];
      for (const k of Object.keys(v)) pairs.push([k, toWire(v[k], seen)]);
      return { Map: pairs };
    } finally {
      seen.delete(v);
    }
  }

  function wireToPlain(w) {
    if (w === "Null" || w === null) return null;
    if (typeof w === "object") {
      if ("Bool" in w) return w.Bool;
      if ("Num" in w) return w.Num;
      if ("Str" in w) return w.Str;
      if ("List" in w) return w.List.map(wireToPlain);
      if ("Map" in w) {
        const obj = {};
        for (const [k, val] of w.Map) obj[k] = wireToPlain(val);
        return obj;
      }
    }
    return null;
  }

  function readResult(r) {
    if (r.t === "scalar") return r.v;
    if (r.t === "handle") return memProxy(r.node, r.kind);
    return undefined;
  }

  function memProxy(node, kind) {
    const key = GEN + ":" + node;
    const hit = handleCache.get(key);
    if (hit !== undefined) return hit;
    const proxy =
      kind === "map" ? new Proxy({}, mapHandler(node)) : new Proxy({}, listHandler(node));
    handleCache.set(key, proxy);
    return proxy;
  }

  // 映射保留方法名：数据键优先（先查实际数据，存在则返回数据），
  // 否则返回绑定方法——名为 keys/size/to_dict 的数据不会被方法遮蔽。
  const MAP_METHOD_KEYS = new Set(["keys", "size", "to_dict"]);

  function mapHandler(node) {
    return {
      get(_t, k) {
        if (typeof k === "symbol") return undefined;
        if (MAP_METHOD_KEYS.has(k)) {
          const probe = rt("mem.map_get", { gen: GEN, node, key: k });
          if (probe.t !== "missing") return readResult(probe);
          if (k === "keys") {
            return function () { return rt("mem.map_keys", { gen: GEN, node }).keys; };
          }
          if (k === "to_dict") {
            return function () { return wireToPlain(rt("mem.to_value", { gen: GEN, node }).value); };
          }
          return rt("mem.map_size", { gen: GEN, node }).value;
        }
        const r = rt("mem.map_get", { gen: GEN, node, key: String(k) });
        return readResult(r);
      },
      set(_t, k, v) {
        if (typeof k === "symbol") throw ipcError("INVALID_VALUE", "memory 不支持 symbol 键");
        rtVoid("mem.map_set", { gen: GEN, node, key: String(k), value: toWire(v) });
        return true;
      },
      deleteProperty(_t, k) {
        if (typeof k === "symbol") return false;
        rtVoid("mem.map_delete", { gen: GEN, node, key: String(k) });
        return true;
      },
      has(_t, k) {
        if (typeof k === "symbol") return false;
        return rt("mem.map_has", { gen: GEN, node, key: String(k) }).value === true;
      },
      ownKeys() {
        return rt("mem.map_keys", { gen: GEN, node }).keys;
      },
      getOwnPropertyDescriptor(_t, k) {
        if (typeof k === "symbol") return undefined;
        const r = rt("mem.map_get", { gen: GEN, node, key: String(k) });
        if (r.t === "missing") return undefined;
        return { value: readResult(r), enumerable: true, writable: true, configurable: true };
      },
    };
  }

  function listHandler(node) {
    return {
      get(_t, k) {
        if (typeof k === "symbol") {
          if (k === Symbol.iterator) return listIterator(node);
          return undefined;
        }
        if (k === "length") return rt("mem.list_size", { gen: GEN, node }).value;
        if (k === "push") {
          return function (...items) {
            for (const it of items) rtVoid("mem.list_append", { gen: GEN, node, value: toWire(it) });
            return rt("mem.list_size", { gen: GEN, node }).value;
          };
        }
        if (k === "to_list") {
          return function () { return wireToPlain(rt("mem.to_value", { gen: GEN, node }).value); };
        }
        if (k === "remove") {
          return function (i) {
            if (!Number.isInteger(i) || i < 0) {
              throw ipcError("INVALID_ARGUMENT", "remove 需要非负整数下标");
            }
            rtVoid("mem.list_remove", { gen: GEN, node, index: i });
          };
        }
        if (/^(0|[1-9][0-9]*)$/.test(k)) {
          return readResult(rt("mem.list_get", { gen: GEN, node, index: parseInt(k, 10) }));
        }
        return undefined;
      },
      set(_t, k, v) {
        if (typeof k === "symbol" || !/^(0|[1-9][0-9]*)$/.test(k)) {
          throw ipcError("INVALID_VALUE", "受控列表只支持数字下标写入");
        }
        rtVoid("mem.list_set", { gen: GEN, node, index: parseInt(k, 10), value: toWire(v) });
        return true;
      },
      has(_t, k) {
        if (typeof k === "symbol" || !/^(0|[1-9][0-9]*)$/.test(k)) return false;
        return rt("mem.list_get", { gen: GEN, node, index: parseInt(k, 10) }).t !== "missing";
      },
      deleteProperty(_t, k) {
        // 删除列表元素请用 remove(i)；下标删除不静默成功。
        if (typeof k !== "symbol" && /^(0|[1-9][0-9]*)$/.test(k)) {
          throw ipcError("INVALID_OPERATION", "受控列表删除请使用 remove(index)");
        }
        return false;
      },
      ownKeys() {
        const n = rt("mem.list_size", { gen: GEN, node }).value;
        const keys = [];
        for (let i = 0; i < n; i++) keys.push(String(i));
        return keys;
      },
      getOwnPropertyDescriptor(_t, k) {
        if (typeof k === "symbol" || !/^(0|[1-9][0-9]*)$/.test(k)) return undefined;
        const r = rt("mem.list_get", { gen: GEN, node, index: parseInt(k, 10) });
        if (r.t === "missing") return undefined;
        return { value: readResult(r), enumerable: true, writable: true, configurable: true };
      },
    };
  }

  function listIterator(node) {
    return function () {
      const entries = rt("mem.list_entries", { gen: GEN, node }).entries;
      const arr = entries.map(readResult);
      let i = 0;
      return {
        next() { return i < arr.length ? { value: arr[i++], done: false } : { done: true }; },
        [Symbol.iterator]() { return this; },
      };
    };
  }

  // ---- Game 对象 ------------------------------------------------------------

  const Game = {
    E: E,
    NORTH: new Position(0, -1),
    SOUTH: new Position(0, 1),
    WEST: new Position(-1, 0),
    EAST: new Position(1, 0),

    robots() { ensureMirror(); return M.robots.map(robotView); },
    shelves() { ensureMirror(); return M.shelves.map(shelfView); },
    chargers() { ensureMirror(); return M.chargers.map(chargerView); },
    ports() { ensureMirror(); return M.ports.map(portView); },
    vehicles(kind) {
      ensureMirror();
      const vs = M.vehicles.map(vehicleView);
      return kind === undefined ? vs : vs.filter(function (v) { return v.kind === kind; });
    },
    ground_boxes() { ensureMirror(); return M.ground_boxes.map(boxView); },
    my_orders() { ensureMirror(); return M.my_orders.map(orderView); },
    objects_at(x, y) {
      ensureMirror();
      const found = [];
      for (const r of M.robots) if (r.x === x && r.y === y) found.push(["robot", r.id, robotView(r)]);
      for (const s of M.shelves) if (s.x === x && s.y === y) found.push(["shelf", s.id, shelfView(s)]);
      for (const c of M.chargers) if (c.x === x && c.y === y) found.push(["charger", c.id, chargerView(c)]);
      for (const p of M.ports) if (p.x === x && p.y === y) found.push(["port", p.id, portView(p)]);
      for (const v of M.vehicles) if (v.x === x && v.y === y) found.push(["vehicle", v.id, vehicleView(v)]);
      for (const b of M.ground_boxes) if (b.x === x && b.y === y) found.push(["box", b.id, boxView(b)]);
      found.sort(function (a, b) { return a[1] - b[1]; });
      return found.map(function (e) { return e[2]; });
    },
    get_object_by_id(id) {
      ensureMirror();
      id = Number(id);
      for (const c of [
        [M.robots, robotView], [M.shelves, shelfView], [M.chargers, chargerView],
        [M.ports, portView], [M.vehicles, vehicleView], [M.ground_boxes, boxView],
      ]) {
        for (const raw of c[0]) if (raw.id === id) return c[1](raw);
      }
      for (const o of M.my_orders) if (o.id === id) return orderView(o);
      for (const o of M.sell_orders) if (o.id === id) return orderView(o);
      for (const o of M.buy_orders) if (o.id === id) return orderView(o);
      return null;
    },
    map_size() { ensureMirror(); return [M.map_w, M.map_h]; },
    get gold() { ensureMirror(); return Number(M.gold_milli) / 1000; },
    get debt() { ensureMirror(); return Number(M.debt_milli) / 1000; },
    get tick() { ensureMirror(); return M.tick; },

    market: {
      sell_orders() { ensureMirror(); return M.sell_orders.map(orderView); },
      buy_orders() { ensureMirror(); return M.buy_orders.map(orderView); },
      take(orderId) {
        const res = rt("market.take", { order_id: Number(orderId) });
        if (res.delta) applyDelta(res.delta);
        return res.code;
      },
      cancel(orderId) {
        const res = rt("market.cancel", { order_id: Number(orderId) });
        if (res.delta) applyDelta(res.delta);
        return res.code;
      },
    },

    // 管理操作（即时生效；docs/game-design/08）。购买属后续里程碑。
    destroy(id) {
      const res = rt("manage.destroy", { target_id: idOf(id) });
      if (res.delta) applyDelta(res.delta);
      return res.code;
    },

    log(...args) {
      const line = args
        .map(function (a) { return typeof a === "string" ? a : JSON.stringify(a); })
        .join(" ");
      rtVoid("log", { line: line });
    },
  };
  Object.defineProperty(Game, "memory", {
    enumerable: false,
    get: function () { return memProxy(0, "map"); },
  });

  globalThis.Game = Game;
})();
