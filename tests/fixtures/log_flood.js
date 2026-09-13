// 持续日志洪泛（配合外部杀进程：验证终止路径不被 Game 请求队列阻塞）。
function loop() {
  let i = 0;
  for (;;) {
    Game.log("spam", i++);
  }
}
