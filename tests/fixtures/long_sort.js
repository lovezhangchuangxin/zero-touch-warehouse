// 不可中断单条长运算：TypedArray.sort 是 C 层 qsort，quickjs-ng 不在其中
// 轮询中断（A0 探针验证）。数组在初始化阶段构建（初始化预算单独）。
const BIG = new Int32Array(32_000_000);
for (let i = 0; i < BIG.length; i++) BIG[i] = ((i * 2654435761) >>> 0);

function loop() {
  Game.memory["sort_started"] = Game.tick;   // 已提交
  BIG.sort();                                // 单条秒级运算 → 主进程看门狗
}
