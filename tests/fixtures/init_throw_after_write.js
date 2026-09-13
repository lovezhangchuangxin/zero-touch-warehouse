// 初始化失败（写 memory 后抛错）：临时分支应被丢弃。
Game.memory["doomed"] = 1;
throw new Error("boom before loop");
