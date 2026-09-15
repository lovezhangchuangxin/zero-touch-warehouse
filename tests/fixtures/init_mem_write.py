# 配合 ZTW_FAULT=abort_init_after_mem：初始化写 memory 后宿主崩溃。
# 与 init_mem_write.js 等价（a1_fault_parity 双语言对账锚点）。
Game.memory["half"] = 1


def loop():
    pass
