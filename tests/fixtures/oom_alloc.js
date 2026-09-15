// 无限分配：触发 JS 堆上限（环境级故障）。
export function loop() {
  Game.memory["oom_mark"] = 1;   // 已提交
  const a = [];
  for (;;) a.push(new Array(200000).fill(1));
}
