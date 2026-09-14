// A1 双语言 memory 值模型矩阵（docs/architecture/08 验证表「memory 值模型」
// 行）。每步以相同字符串记录结果（ok / err:<CODE> / 值），供 Rust 侧逐条
// 对账——与 matrix_memory.py 必须产出完全相同的日志序列与终态。
function step(name, fn) {
  try {
    fn();
    Game.log("ok:" + name);
  } catch (e) {
    Game.log("err:" + name + ":" + e.code);
  }
}

const alias = [1, 2];
Game.memory["alias"] = alias; // 写入深拷贝
alias.push(3); // 陷阱：原生数组后续变化不影响 memory
Game.memory["alias"].push(4); // 受控句柄追加 → 提交

Game.memory["nest"] = { a: { b: [1] } };
Game.memory["nest"]["a"]["b"].push(2);
Game.memory["nest"]["a"]["c"] = 5;

// 失效句柄：持有 nest.a 后整体替换 nest，旧句柄操作必须被拒。
const h = Game.memory["nest"]["a"];
Game.memory["nest"] = 1;
step("stale", function () {
  h["b"].push(3);
});

// 字符串数字键：插入序保持（"10"/"1"/"02"/"2" 不得按数值重排）。
Game.memory["k"] = {};
Game.memory["k"]["10"] = "a";
Game.memory["k"]["1"] = "b";
Game.memory["k"]["02"] = "c";
Game.memory["k"]["2"] = "d";

// 整数与数值边界（docs 06 值模型：±(2^53-1)）。
step("big_ok", function () {
  Game.memory["big"] = 9007199254740991;
});
step("big_over", function () {
  Game.memory["big"] = 9007199254740992;
});
step("big_neg_ok", function () {
  Game.memory["big"] = -9007199254740991;
});
step("nonfinite", function () {
  Game.memory["nf"] = Infinity;
});

// 循环引用拒绝。
step("cycle", function () {
  const c = [];
  c.push(c);
  Game.memory["cyc"] = c;
});

// Position 与普通数组往返。
Game.memory["pos"] = Game.NORTH;
Game.memory["pos2"] = [3, 4];

// 跨执行句柄：init 顶层持有句柄，提交使代次 +1，loop 中使用必须按
// STALE_MEMORY_REFERENCE 拒绝（docs 06「提交即重建句柄」——两语言同结果）。
Game.memory["held"] = { x: 0 };
const held = Game.memory["held"];

function loop() {
  step("held_stale", function () {
    held["x"] = 1;
  });
  Game.log("held_now " + Game.memory["held"]["x"]);
  Game.log("pos " + Game.memory["pos"][0] + "," + Game.memory["pos"][1]);
  Game.log("kkeys " + Game.memory["k"].keys().join(","));
  Game.log("alias " + Game.memory["alias"].length);
  // 有序枚举对账以 map_keys 为准（docs 06：相同有序枚举）；to_dict 在
  // JS 侧受原生对象“数字样键重排”语义影响，不做字符串级对照。
  Game.log("alias_all " + JSON.stringify(Game.memory["alias"].to_list()));
}
