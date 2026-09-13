// 吞中断尝试：try/catch 包住忙循环。
// quickjs-ng 的中断异常不可捕获（A0 探针已验证）；本脚本验证即便如此
// 写法，会话仍在时限内以脚本级错误或看门狗终止收场。
function loop() {
  Game.memory["sw"] = 1;
  try {
    let s = 0;
    for (;;) { s += JSON.stringify({ a: s }).length; }
  } catch (e) {
    Game.log("swallowed", e.name);  // 不应到达
  }
  Game.log("survived");             // 不应到达
}
