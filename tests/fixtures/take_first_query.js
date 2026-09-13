// take 先于任何查询：配合 ZTW_FAULT=skip_delta_replay 才真正踩到
// “增量被丢弃 → 下一次查询经 mirror.fetch 整体重建”的路径。
function loop() {
  if (Game.tick === 0 && Game.market.sell_orders().length > 0) {
    const id = Game.market.sell_orders()[0].id;  // 预检查询（会先重建一次镜像）
    const code = Game.market.take(id);           // 增量被注入丢弃 → stale
    const after = Game.my_orders().length;       // 重建后查询
    const listings = Game.market.sell_orders().length + Game.market.buy_orders().length;
    Game.log("take", code, "after", after, "listings", listings, "gold", Game.gold);
  }
}
