// 配合 ZTW_FAULT=dup_request_corrupt:1：log（第 1 号请求）回复后以同号
// 异负载重发，预期主进程按协议故障终止宿主。
export function loop() {
  Game.log("first");
}
