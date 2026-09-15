// 受控 memory 用法与“深拷贝陷阱”演示（docs/architecture/06）。
Game.memory["scalar"] = 42;
Game.memory["note"] = "hello";

const items = [];                 // 原生数组
Game.memory["items"] = items;     // 写入深拷贝
items.push(1);                    // 陷阱：原生数组后续变化不影响 memory

Game.memory["items"].push(2);     // 受控句柄追加 → 提交
Game.memory["nested"] = { a: { b: [1, 2] } };
Game.memory["nested"]["a"]["c"] = 3;   // 嵌套句柄写 → 逐层提交

export function loop() {
  Game.memory["items"].push(Game.tick);
  Game.memory["count"] = Game.memory["items"].length;
  const r = Game.robots()[0];
  r.memory["visited"] = (r.memory["visited"] || 0) + 1;
  Game.log("keys", Game.memory.keys().join(","));
}
