// 配合 request_limit_per_exec 小值配置：持续 Game.log，触限时绑定层抛错
// 且不捕获——错误冒泡成 Fault 帧，主进程须把结局改判为 REQUEST_LIMIT
//（与捕获后正常完成的 request_limit.js 路径同款语义，docs 03）。
export function loop() {
  for (let i = 0; i < 10; i++) {
    Game.log("y" + i);
  }
}
