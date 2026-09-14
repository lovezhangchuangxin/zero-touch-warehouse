# take 先于任何查询：配合 ZTW_FAULT=skip_delta_replay 才真正踩到
# "增量被丢弃 → 下一次查询经 mirror.fetch 整体重建"的路径。
# 与 take_first_query.js 产出同构日志（take/after/listings 锚点）。
def loop():
    if Game.tick == 0 and len(Game.market.sell_orders()) > 0:
        ident = Game.market.sell_orders()[0].id  # 预检查询（会先重建一次镜像）
        code = Game.market.take(ident)  # 增量被注入丢弃 → stale
        after = len(Game.my_orders())  # 重建后查询
        listings = len(Game.market.sell_orders()) + len(Game.market.buy_orders())
        Game.log("take", code, "after", after, "listings", listings, "gold", Game.gold)
