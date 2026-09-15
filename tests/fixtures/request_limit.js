// 配合 request_limit_per_exec 小值配置：持续 Game.log 直到被拒。
// 首个错误即退出，让宿主正常走到完成帧——主进程把结局改判为超限暂停。
export function loop() {
  for (let i = 0; i < 10; i++) {
    try {
      Game.log("x" + i);
    } catch (e) {
      break;
    }
  }
}
