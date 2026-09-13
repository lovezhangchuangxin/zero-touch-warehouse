// 配合 ZTW_FAULT=dup_request:2：take（第 2 号请求）收到回复后逐字节重发。
// 预期：主进程去重缓存返回原结果，不重复扣款 / 接单。
Game.memory["t"] = 0; // 请求 #1（init 阶段）
function loop() {
  const asks = Game.market.sell_orders();
  if (asks.length > 0) Game.market.take(asks[0].id); // 请求 #2
}
