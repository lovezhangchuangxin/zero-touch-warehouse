// 配合 ZTW_FAULT=abort_after_reply:market.take：接单回复送达后宿主硬崩溃。
export function loop() {
  const asks = Game.market.sell_orders();
  if (asks.length > 0) {
    Game.market.take(asks[0].id);
  }
}
