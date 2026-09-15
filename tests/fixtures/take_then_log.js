// 配合 ZTW_FAULT=abort_before_send:log：take 已提交，log 在发出前 abort，
// 主进程从未见到该请求（提交前断连点）。
export function loop() {
  const asks = Game.market.sell_orders();
  if (asks.length > 0) Game.market.take(asks[0].id);
  Game.log("after take");
}
