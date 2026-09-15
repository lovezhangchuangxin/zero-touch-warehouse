// 配合 ZTW_FAULT=dup_request:1 / dup_request_corrupt:1：log（第 1 号请求）
// 收到回复后的重发注入（同负载去重回放 / 同号异负载协议故障）。
// 与 dup_log.py 等价（a1_fault_parity 对账锚点）。
export function loop() {
  Game.log("first");
}
