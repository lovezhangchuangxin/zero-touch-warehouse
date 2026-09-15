// M3 校准 · 快转策略：只做 water，紧阈值低买高卖、地面暂存、即卸即卖。
// 三策略对比之一（records/m3-calibration.md；地图 m3-trade）。

var GOODS = ["water"];
// 买卖门深度（相对中枢 mid 的慢速 EMA）：往返税 ≈ 半价差 ×2，须等
// 基准价 ±1σ 量级的深坑 / 高峰才进出场，按货物波动档定深度。
var GATE = {
  water: { buy: 0.93, sell: 1.03 },
  battery: { buy: 0.9, sell: 1.08 },
  chip: { buy: 0.85, sell: 1.12 },
};

var MARGIN = 1.03; // 卖出保底：bid ≥ 摊薄成本 × MARGIN
var MAX_OPEN = 2; // 并发在途订单上限（一进一出）
var RESERVE = 30; // 现金垫底（gold）

// 地面暂存格（装卸位与货架间的空地）。
// 分区作业：0 号机管西装卸位（x<8）、1 号机管东装卸位；暂存贴北墙
// 一行（避开装卸位第二格），沿 y=2 通道作业不被箱堵。
var ZONES = [
  {
    park: [
      [1, 1],
      [2, 1],
      [4, 1],
      [5, 1],
      [6, 1],
    ],
    shelfY: 3,
    stand: [6, 9],
    idle: [4, 4],
  },
  {
    park: [
      [8, 1],
      [9, 1],
      [10, 1],
      [11, 1],
      [13, 1],
    ],
    shelfY: 6,
    stand: [7, 9],
    idle: [8, 4],
  },
];

function stepToward(r, tx, ty) {
  const m = Game.memory;
  const dk = "det" + r.id;
  let det = m[dk]; // 绕行方向记忆：主轴被挡时沿同一方向连续绕行（防对穿横跳）
  if (!det || typeof det[0] !== "number" || typeof det[1] !== "number") det = null;
  const dx = tx - r.pos.x,
    dy = ty - r.pos.y;
  const sx = Math.sign(dx),
    sy = Math.sign(dy);
  const primary =
    Math.abs(dx) >= Math.abs(dy)
      ? [
          [sx, 0],
          [0, sy],
        ]
      : [
          [0, sy],
          [sx, 0],
        ];
  const side =
    sx === 0
      ? [
          [1, 0],
          [-1, 0],
        ]
      : [
          [0, 1],
          [0, -1],
        ];
  for (const [mx, my] of [...(det ? [det] : []), ...primary, ...side]) {
    if ((mx !== 0 || my !== 0) && r.move([mx, my]) === Game.E.OK) {
      const isPrimary = primary.some(([px, py]) => px === mx && py === my);
      m[dk] = isPrimary ? null : [mx, my];
      return true;
    }
  }
  return false;
}

function at(r, x, y) {
  return r.pos.x === x && r.pos.y === y;
}

// 走到 (x,y) 旁并执行 act；优先无静态占用的站位，未到位走一步
// （贪心 + 侧绕）。
function doAdjacent(r, x, y, act) {
  const opts = [
    [x, y + 1],
    [x, y - 1],
    [x + 1, y],
    [x - 1, y],
  ];
  for (const [sx, sy] of opts) {
    if (at(r, sx, sy)) return act();
  }
  let target = opts[0];
  for (const [sx, sy] of opts) {
    if (Game.objects_at(sx, sy).length === 0) {
      target = [sx, sy];
      break;
    }
  }
  stepToward(r, target[0], target[1]);
  return null;
}

// 价格中枢：每 tick 以两侧挂单均值（ask 均价与 bid 均价的中点）的慢速
// EMA 估计锚价（0.5%/tick，约 200 tick 记忆——跨数个回归周期，长趋势中
// 不追价）。
function midOf(g) {
  // 两侧均值（而非极值）：吃掉最便宜 ask 不抬高中枢，估计不被自己的
  // 买入行为偏移。
  let askSum = 0;
  let askN = 0;
  let bidSum = 0;
  let bidN = 0;
  for (const o of Game.market.sell_orders()) {
    if (o.goods_type !== g) continue;
    askSum += o.unit_price;
    askN++;
  }
  for (const o of Game.market.buy_orders()) {
    if (o.goods_type !== g) continue;
    bidSum += o.unit_price;
    bidN++;
  }
  return askN === 0 || bidN === 0 ? null : (askSum / askN + bidSum / bidN) / 2;
}

function updateEmas() {
  const m = Game.memory;
  if (!m.seen) {
    m.seen = 1;
    m.mid = {};
    m.cost = {};
    m.q = {};
  }
  for (const g of GOODS) {
    const mid = midOf(g);
    if (mid === null) continue;
    const e = m.mid[g];
    m.mid[g] = e === undefined ? mid : e * 0.995 + mid * 0.005;
  }
}

function freeParking() {
  let n = 0;
  for (const z of ZONES) n += freeParkingOf(z);
  return n;
}

function freeParkingOf(zone) {
  const occ = new Set();
  for (const b of Game.ground_boxes()) occ.add(b.location.x + "," + b.location.y);
  let n = 0;
  for (const [cx, cy] of zone.park) if (!occ.has(cx + "," + cy)) n++;
  for (const s of shelvesOf(zone)) n += s.capacity - s.boxes.length;
  return n;
}

function shelvesOf(zone) {
  return Game.shelves().filter((s) => s.pos.y === zone.shelfY);
}

function stock(g) {
  let n = 0;
  for (const b of Game.ground_boxes()) if (b.goods_type === g) n++;
  for (const s of Game.shelves()) {
    for (const b of s.boxes) if (b.goods_type === g) n++;
  }
  for (const rr of Game.robots()) if (rr.carry && rr.carry.goods_type === g) n++;
  return n;
}

function pendingBuy(g) {
  let n = 0;
  for (const o of Game.my_orders()) {
    if (o.side === "sell" && o.goods_type === g) n += o.qty;
  }
  return n;
}

// 在途买入总量（全货物）：容量门按总箱数计——暂存格与货架槽不分货物。
function pendingBuyAll() {
  let n = 0;
  for (const o of Game.my_orders()) {
    if (o.side === "sell") n += o.qty;
  }
  return n;
}

function openBuys() {
  let n = 0;
  for (const o of Game.my_orders()) {
    if (o.side === "sell") n++;
  }
  return n;
}

function tradePolicy() {
  const m = Game.memory;
  for (const o of Game.market.sell_orders()) {
    if (openBuys() >= MAX_OPEN) break; // 停止继续买入，不阻断卖出扫描
    if (GOODS.indexOf(o.goods_type) < 0) continue;
    const gate = GATE[o.goods_type];
    const mid = m.mid[o.goods_type];
    if (!gate || mid === undefined || o.unit_price > mid * gate.buy) continue;
    if (Game.gold < o.unit_price * o.qty + RESERVE) continue;
    if (o.qty > freeParking() - pendingBuyAll()) continue;
    if (Game.market.take(o.id) === Game.E.OK) {
      const g = o.goods_type;
      const q0 = m.q[g] || 0;
      m.cost[g] = ((m.cost[g] || 0) * q0 + o.unit_price * o.qty) / (q0 + o.qty);
      m.q[g] = q0 + o.qty;
      if (openBuys() >= MAX_OPEN) break;
    }
  }
  for (const o of Game.market.buy_orders()) {
    const g = o.goods_type;
    if (GOODS.indexOf(g) < 0) continue;
    const gate = GATE[g];
    const mid = m.mid[g];
    if (!gate || mid === undefined) continue;
    const need = Math.max(mid * gate.sell, (m.cost[g] || 1e9) * MARGIN);
    if (o.unit_price < need) continue;
    if (stock(g) + pendingBuy(g) < o.qty) continue;
    if (Game.market.take(o.id) === Game.E.OK) {
      m.q[g] = (m.q[g] || 0) - o.qty;
    }
  }
}

// 携带物落位：本分区货架顺路清运，满架落地面暂存。
function parkCarry(r, zone) {
  for (const s of shelvesOf(zone)) {
    if (s.boxes.length < s.capacity) {
      doAdjacent(r, s.pos.x, s.pos.y, () => r.give(s));
      return;
    }
  }
  const occ = new Set();
  for (const b of Game.ground_boxes()) occ.add(b.location.x + "," + b.location.y);
  for (const [cx, cy] of zone.park) {
    if (occ.has(cx + "," + cy)) continue;
    doAdjacent(r, cx, cy, () => r.drop(cx, cy));
    return;
  }
}

// 找一箱指定货物去取：地面暂存优先、本分区货架兜底。
function findBoxToFetch(g, zone) {
  for (const [cx, cy] of zone.park) {
    for (const b of Game.ground_boxes()) {
      if (b.goods_type === g && b.location.x === cx && b.location.y === cy) return { x: cx, y: cy };
    }
  }
  // 对侧分区暂存兜底：充电 / 绕行导致的偶发停靠会把箱落在对侧暂存行，
  // 只扫本分区会取不到（直到货架兜底都取不到）。
  for (const z of ZONES) {
    if (z === zone) continue;
    for (const [cx, cy] of z.park) {
      for (const b of Game.ground_boxes()) {
        if (b.goods_type === g && b.location.x === cx && b.location.y === cy)
          return { x: cx, y: cy };
      }
    }
  }
  for (const s of shelvesOf(zone)) {
    for (const b of s.boxes) {
      if (b.goods_type === g) return { shelf: s, box: b };
    }
  }
  return null;
}

function maybeCharge(r, zone) {
  const m = Game.memory;
  const key = "chg" + r.id;
  if (m[key] === undefined) m[key] = 0;
  if (r.energy < r.energy_max * 0.2) m[key] = 1;
  if (r.energy >= r.energy_max * 0.9) m[key] = 0;
  if (!m[key]) return false;
  if (at(r, zone.stand[0], zone.stand[1])) {
    r.charge();
  } else {
    stepToward(r, zone.stand[0], zone.stand[1]);
  }
  return true;
}

function robotWork(r, zone) {
  const vouts = Game.vehicles("out").filter((v) => v.interact_pos.x < 8 === (zone.stand[0] === 6));
  const vins = Game.vehicles("in").filter((v) => v.interact_pos.x < 8 === (zone.stand[0] === 6));

  // 卖出优先：出库车在场就装。
  if (vouts.length > 0) {
    const v = vouts[0];
    if (r.carry) {
      if (r.carry.goods_type === v.goods_type) {
        doAdjacent(r, v.interact_pos.x, v.interact_pos.y, () => r.give(v));
        return;
      }
      parkCarry(r, zone);
      return;
    }
    const spot = findBoxToFetch(v.goods_type, zone);
    if (spot) {
      if (spot.shelf) {
        doAdjacent(r, spot.shelf.pos.x, spot.shelf.pos.y, () => r.take(spot.shelf, spot.box.id));
      } else {
        doAdjacent(r, spot.x, spot.y, () => r.pick(spot.x, spot.y));
      }
      return;
    }
  }

  // 卸货：入库车有箱就卸到暂存。
  if (vins.length > 0 && vins[0].boxes.length > 0) {
    const v = vins[0];
    if (r.carry) {
      parkCarry(r, zone);
      return;
    }
    doAdjacent(r, v.interact_pos.x, v.interact_pos.y, () => r.take(v, v.boxes[0].id));
    return;
  }

  if (!at(r, zone.idle[0], zone.idle[1])) stepToward(r, zone.idle[0], zone.idle[1]);
}

export function loop() {
  updateEmas();
  tradePolicy();
  Game.robots().forEach((r, i) => {
    const zone = ZONES[Math.min(i, ZONES.length - 1)];
    if (maybeCharge(r, zone)) return;
    robotWork(r, zone);
  });
}
