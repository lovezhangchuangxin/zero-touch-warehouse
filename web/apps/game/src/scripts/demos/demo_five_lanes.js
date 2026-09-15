// 五机分流（把 naive 的堵塞改掉，同地图对比吞吐）。
// naive 的堵塞来自五台抢同一条窄道、同一个交互位。分流版两个决定：
// 1) 让出主通道：四台待命机开场一次性同向东进（各自专属路线与终点，
//    纵队行军互不对穿），就位后永久静止——空着的机器人也是路障；
// 2) 串行流水：worker0 独占主工作区，按"接卖单→卸车暂存→接买单→
//    逐箱交付"单机闭环流程做完全部订单（本图两个装卸位窄道，并行卸车只会
//    互相穿行——错开任务比抢并行更能提高吞吐）。
const PARK = [
  [2, 5, 2, 4],
  [2, 3, 2, 4],
  [3, 5, 3, 4],
  [3, 3, 3, 4],
  [4, 5, 4, 4],
];

// 待命机专属行军路线（终点都在货架东侧 x=8 的空旷区，且避开 worker 的
// 动脉——row2/row4 两条东西向走廊与 3/12 两列装卸位通道）。
// robot2/4 先纵一格到无障碍行再东进（y=6 行有货架、y=10 行有充电桩）。
const DOCK_ROUTE = {
  1: [[8, 5]],
  2: [
    [2, 7],
    [8, 7],
  ],
  3: [[8, 8]],
  4: [
    [2, 9],
    [8, 9],
  ],
};
const dockDone = {};
// 各待命机已到达的路标下标（按进度推进，避免走过头后被首个路标拉回）。
const dockStep = { 1: 0, 2: 0, 3: 0, 4: 0 };

function stepToward(r, tx, ty) {
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
  // 同行 / 同列被挡（如暂存区的箱子）时向垂直轴侧向绕一步。
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
  for (const [mx, my] of [...primary, ...side]) {
    if ((mx !== 0 || my !== 0) && r.move([mx, my]) === Game.E.OK) return true;
  }
  return false;
}

function standNear(v) {
  return [v.interact_pos.x, v.interact_pos.y + 1];
}

function parkBox(r) {
  const occ = new Set();
  for (const b of Game.ground_boxes()) occ.add(b.location.x + "," + b.location.y);
  for (const [cx, cy, sx, sy] of PARK) {
    if (occ.has(cx + "," + cy)) continue;
    if (r.pos.x === sx && r.pos.y === sy) {
      r.drop(cx, cy);
      return;
    }
    stepToward(r, sx, sy);
    return;
  }
}

// 待命机行军：沿专属 waypoint 前进（按 dockStep 记录进度），全部就位后静止。
function dockDrive(r, i) {
  const route = DOCK_ROUTE[i];
  if (dockDone[i] || !route) return;
  const k = dockStep[i] || 0;
  if (k >= route.length) {
    dockDone[i] = true;
    return;
  }
  const [wx, wy] = route[k];
  if (r.pos.x === wx && r.pos.y === wy) {
    dockStep[i] = k + 1;
    return;
  }
  stepToward(r, wx, wy);
}

function work(r) {
  const vin = Game.vehicles("in")[0];
  const vout = Game.vehicles("out")[0];

  if (vout) {
    if (r.carry && r.carry.goods_type !== vout.goods_type) {
      parkBox(r);
      return;
    }
    if (r.carry) {
      const [tx, ty] = standNear(vout);
      if (r.pos.x === tx && r.pos.y === ty) {
        r.give(vout);
        return;
      }
      stepToward(r, tx, ty);
      return;
    }
    const boxes = Game.ground_boxes().filter(function (b) {
      return b.goods_type === vout.goods_type;
    });
    if (!boxes.length) return;
    const b = boxes[0];
    const bx = b.location.x,
      by = b.location.y;
    if (Math.abs(r.pos.x - bx) + Math.abs(r.pos.y - by) === 1) {
      r.pick(bx, by);
      return;
    }
    stepToward(r, bx, by);
    return;
  }

  if (vin) {
    if (r.carry) {
      parkBox(r);
      return;
    }
    const [tx, ty] = standNear(vin);
    if (r.pos.x === tx && r.pos.y === ty) {
      r.take(vin, vin.boxes[0].id);
      return;
    }
    stepToward(r, tx, ty);
    return;
  }

  const asks = Game.market.sell_orders();
  if (asks.length) {
    Game.market.take(asks[0].id);
    return;
  }
  const bids = Game.market.buy_orders();
  if (bids.length) {
    Game.market.take(bids[0].id);
    return;
  }
}

export function loop() {
  const rs = Game.robots();
  for (let i = 1; i < rs.length; i++) dockDrive(rs[i], i);
  work(rs[0]);
}
