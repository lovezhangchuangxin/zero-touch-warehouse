// 五机贪心：无分工、无分道。每台都独立执行与单机版相同的驱动——
// 抢同一张挂单、挤同一个交互站位、共用同一个暂存区。现象（事件面板
// 可解释）：抢单 ORDER_TAKEN、站位互挤 CELL_OCCUPIED/CELL_CONTESTED、
// 通道对穿 CHAIN_BLOCKED、暂存格抢占。
function standNear(v) {
  return [v.interact_pos.x, v.interact_pos.y + 1];
}

function stepToward(r, tx, ty) {
  const dx = tx - r.pos.x,
    dy = ty - r.pos.y;
  const sx = Math.sign(dx),
    sy = Math.sign(dy);
  const tries =
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
  for (const [mx, my] of [...tries, ...side]) {
    if ((mx !== 0 || my !== 0) && r.move([mx, my]) === Game.E.OK) return true;
  }
  return false;
}

const PARK = [
  [2, 5, 2, 4],
  [2, 3, 2, 4],
  [3, 5, 3, 4],
  [3, 3, 3, 4],
  [4, 5, 4, 4],
];

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

function drive(r, vin, vout) {
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

function loop() {
  const vin = Game.vehicles("in")[0];
  const vout = Game.vehicles("out")[0];
  for (const r of Game.robots()) drive(r, vin, vout);
}
