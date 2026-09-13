// 接单与记账间崩溃（脚本级）：订单成立、memory 记录缺失。
function loop() {
  const asks = Game.market.sell_orders();
  if (asks.length > 0 && Game.my_orders().length === 0) {
    const code = Game.market.take(asks[0].id);
    Game.log("took", code);
    throw new Error("bookkeeping lost");
  }
}
