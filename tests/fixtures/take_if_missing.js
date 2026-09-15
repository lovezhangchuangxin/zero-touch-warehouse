// 配合 ZTW_FAULT=abort_after_send：take 请求发出后立即 abort（提交后、
// 回复前断连点）。重启后本程序先查询真实订单再接单，不重放调用——
// 若崩溃前 take 已提交则不再接第二单。
export function loop() {
  if (Game.my_orders().length === 0) {
    const asks = Game.market.sell_orders();
    if (asks.length > 0) Game.market.take(asks[0].id);
  }
}
