// 无限循环（无 try/catch）：运行时内中断应直接终止执行。
function loop() {
  Game.memory["before"] = 1;   // 已提交写，故障后必须保留
  Game.log("about to loop");
  let s = 0;
  for (;;) s++;
}
