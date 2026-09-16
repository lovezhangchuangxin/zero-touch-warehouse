// Game API 补全数据（docs/architecture/05「补全与悬浮文档由数据驱动、
// 编辑器只做展示」）。本期在前端建数据模块；不在 crates/api 建 Rust
// 生成器（明确不做项）。
//
// 权威来源（手动同步，将来由 crates/api 同源生成——对齐
// @ztw/game-types 现行做法）：
//   - crates/api/bindings/bootstrap.js / bootstrap.py —— 绑定层唯一实现；
//   - web/packages/game-types/src/index.d.ts —— 玩家侧 TS 声明；
//   - docs/game-design/08-api-design.md —— 文案出处（含总则）。
// 结果码清单与中文说明经 ../codes.ts 消费，不在本文件复述。
//
// 文案面向补全悬浮框：signature 展示用（右侧 detail），doc 为正文，
// 二者均为纯文本（CM6 info 按纯文本渲染）。

/** 结果码的通用反馈模型说明（docs/game-design/08 总则 5）。 */
export const RESULT_CODE_NOTE =
  "动作调用立即返回受理码，结算结果下一 tick 查 r.last_result；" +
  "管理操作即时生效、立即返回准确结果。比较一律使用 Game.E 常量，避免手打字符串。";

/** Game 根对象的说明（08 总则 1「一切经 Game」）。 */
export const GAME_ROOT_DOC =
  "游戏 API 唯一入口：查询、管理操作、辅助工具和常量都挂在 Game 上。" +
  "唯一例外是入口函数 loop()。输入 Game. 查看全部能力。";

export interface ApiMember {
  name: string;
  kind: "method" | "property" | "constant" | "namespace";
  /** 展示签名（补全列表右侧 detail），如 `vehicles(kind?) → Vehicle[]`。 */
  signature: string;
  /** CM6 snippet 模板（方法类带参数时提供；占位符 ${name}）。 */
  template?: string;
  /** 中文文档（补全 info 与悬浮文档共用）。 */
  doc: string;
  /** 命名空间成员（如 Game.market）。 */
  members?: ApiMember[];
}

export const GAME_MEMBERS: ApiMember[] = [
  {
    name: "E",
    kind: "namespace",
    signature: "结果码表",
    doc: "结果码表：受理 / 结算 / 管理共用一套大写蛇形词表。" + RESULT_CODE_NOTE,
    members: [], // 结果码经 codes.ts 生成，见 completion.ts
  },
  {
    name: "NORTH",
    kind: "constant",
    signature: "NORTH → Position(0, -1)",
    doc: "方向常量：上。用作 r.move(Game.NORTH) 等移动参数。",
  },
  {
    name: "SOUTH",
    kind: "constant",
    signature: "SOUTH → Position(0, 1)",
    doc: "方向常量：下。用作 r.move(Game.SOUTH) 等移动参数。",
  },
  {
    name: "WEST",
    kind: "constant",
    signature: "WEST → Position(-1, 0)",
    doc: "方向常量：左。用作 r.move(Game.WEST) 等移动参数。",
  },
  {
    name: "EAST",
    kind: "constant",
    signature: "EAST → Position(1, 0)",
    doc: "方向常量：右。用作 r.move(Game.EAST) 等移动参数。",
  },
  {
    name: "robots",
    kind: "method",
    signature: "robots() → Robot[]",
    template: "robots()",
    doc:
      "全部机器人视图：id / pos / energy / energy_max / carry / last_result / memory，" +
      "动作方法 move、move_to、charge、take、give、pick、drop。" +
      "基于 tick 快照，机器人动作不改变本 tick 查询结果。",
  },
  {
    name: "shelves",
    kind: "method",
    signature: "shelves() → Shelf[]",
    template: "shelves()",
    doc:
      "全部货架视图：id / pos / boxes（Box[]，基线容量 4）/ capacity。" +
      "货架不区分货位，存储是「格子里最多 4 箱」的集合语义，货物以 id 寻址。",
  },
  {
    name: "chargers",
    kind: "method",
    signature: "chargers() → Charger[]",
    template: "chargers()",
    doc: "全部充电桩视图：id / pos。r.charge() 向相邻 id 最小的桩申请本 tick 充电。",
  },
  {
    name: "docks",
    kind: "method",
    signature: "docks() → Dock[]",
    template: "docks()",
    doc:
      "全部装卸位视图：id / pos / ext / docked_vehicle。" +
      "锚点格在墙上，ext 指向库内第二格（占地 1×2、两格均为障碍）。",
  },
  {
    name: "vehicles",
    kind: "method",
    signature: "vehicles(kind?) → Vehicle[]",
    template: "vehicles()",
    doc:
      '占用装卸位的车辆视图：id / kind（"in" 入库、"out" 出库）/ goods_type / ' +
      "interact_pos / order_id / dock / boxes。kind 过滤车型，缺省返回全部。" +
      "车辆接单后出现并占用装卸位，离场后从列表消失。",
  },
  {
    name: "ground_boxes",
    kind: "method",
    signature: "ground_boxes() → Box[]",
    template: "ground_boxes()",
    doc:
      "地面上的货物：id / goods_type / holder（所在容器 id，地面为 null）/" +
      "location（Position）。每箱占一格，坐标即唯一寻址。",
  },
  {
    name: "my_orders",
    kind: "method",
    signature: "my_orders() → Order[]",
    template: "my_orders()",
    doc: "已接未完成的订单，vehicle / dock 字段已填充（市场订单中为 null）。",
  },
  {
    name: "objects_at",
    kind: "method",
    signature: "objects_at(x, y) → Object[]",
    template: "objects_at(${x}, ${y})",
    doc:
      "该格上的全部对象（机器人 / 货架 / 充电桩 / 装卸位 / 车辆 / 地面货物），" + "空格返回 []。",
  },
  {
    name: "get_object_by_id",
    kind: "method",
    signature: "get_object_by_id(id) → Object | null",
    template: "get_object_by_id(${id})",
    doc:
      "按 id 解析任意对象（id 跨类型唯一），无此 id 返回 null。" +
      "凡接受 id 的参数同样接受对象本身（取其 id）。",
  },
  {
    name: "map_size",
    kind: "method",
    signature: "map_size() → [宽, 高]",
    template: "map_size()",
    doc: "地图尺寸 [宽, 高]。坐标原点左上，x 向右，y 向下。",
  },
  {
    name: "find_path",
    kind: "method",
    signature: "find_path(start, goal, opts?) → Position[] | [] | null",
    template: "find_path(${start}, ${goal})",
    doc:
      "静态障碍最短路径（纯查询，不消耗行动机会）：返回不含 start 的最短路径；" +
      "已在到达范围内返回 []；不可达返回 null。障碍是边界、货架、充电桩、" +
      "装卸位占地与地面货物，机器人不算——寻路回答「物理可达」，动态避让由玩家负责。" +
      "start / goal 接受坐标或对象（货架 / 桩 / 装卸位 / 机器人取 pos，车辆取 " +
      "interact_pos，地面货物取所在格）。opts.range 为到达判定半径（正交距离，" +
      "默认 0）。调用受节点预算约束（20 000 次扩展，正常地图不会触及），超限抛" +
      "可读错误而非返回结果。空值判断须显式三分支：JS 的 [] 为 truthy、Python 的" +
      "空列表为 falsy，一律以 === null / is None 判不可达、空列表判「无需移动」。",
  },
  {
    name: "gold",
    kind: "property",
    signature: "gold → number",
    doc:
      "当前金币（显示值）。内部按定点整数存储与判定；" +
      "资金判断勿用浮点等值比较，权威结果以接口返回码为准。",
  },
  {
    name: "debt",
    kind: "property",
    signature: "debt → number",
    doc: "当前欠款（含累计利息）。每 tick 结算完成后按欠款复利计息。",
  },
  {
    name: "tick",
    kind: "property",
    signature: "tick → number",
    doc: "当前 tick 序号。",
  },
  {
    name: "market",
    kind: "namespace",
    signature: "市场（挂单查询 / 接单 / 取消）",
    doc:
      "市场：sell_orders 卖单（对方卖出，玩家买入，车送货来）、" +
      "buy_orders 买单（对方收购，玩家卖出，车来取货）、take 接单、cancel 取消。",
    members: [
      {
        name: "sell_orders",
        kind: "method",
        signature: "sell_orders() → Order[]",
        template: "sell_orders()",
        doc: "全部卖单：对方卖出，玩家买入，车送货来。字段：id / side / goods_type / qty / unit_price。",
      },
      {
        name: "buy_orders",
        kind: "method",
        signature: "buy_orders() → Order[]",
        template: "buy_orders()",
        doc: "全部买单：对方收购，玩家卖出，车来取货。字段同卖单。",
      },
      {
        name: "take",
        kind: "method",
        signature: "take(orderId) → GameCode",
        template: "take(${orderId})",
        doc:
          "接单（管理操作，即时生效）：预留空装卸位，下一 tick 车到；买入即扣款。" +
          "无空闲装卸位 → NO_FREE_DOCK，挂单已离开市场 → ORDER_GONE。" +
          "参数接受订单 id 或订单对象。",
      },
      {
        name: "cancel",
        kind: "method",
        signature: "cancel(orderId) → GameCode",
        template: "cancel(${orderId})",
        doc:
          "取消已接订单（管理操作）：收手续费、释放装卸位。" +
          "车辆未恢复出现时状态 → GOODS_MOVED，挂单已不在 → ORDER_GONE。",
      },
    ],
  },
  {
    name: "buy",
    kind: "method",
    signature: "buy(kind, x, y) → GameCode",
    template: "buy(${kind}, ${x}, ${y})",
    doc:
      '购买设备（管理操作，即时建成扣款）。kind："robot" / "shelf" / "charger" / "dock"。' +
      "装卸位朝向由锚点边界墙唯一推导，角格或非墙格 → NOT_ON_WALL；" +
      "落点被占 → CELL_OCCUPIED，金币不足 → NO_FUNDS。" +
      "同一 tick 多个管理操作按调用顺序生效，新购对象当 tick 即可查询使用。",
  },
  {
    name: "borrow",
    kind: "method",
    signature: "borrow(amount) → GameCode",
    template: "borrow(${amount})",
    doc:
      "借款即时到账（管理操作），受信用额度限制，超额 → CREDIT_EXCEEDED。" +
      "每 tick 结算后按欠款复利计息，Game.debt 即时可见。",
  },
  {
    name: "repay",
    kind: "method",
    signature: "repay(amount) → GameCode",
    template: "repay(${amount})",
    doc:
      "归还部分或全部欠款（管理操作），金额以欠款为上限，超额部分无效。" + "金币不足 → NO_FUNDS。",
  },
  {
    name: "destroy",
    kind: "method",
    signature: "destroy(id) → GameCode",
    template: "destroy(${id})",
    doc:
      "销毁对象或货物（管理操作）。前提不满足时失败：" +
      "货架含货 → NOT_EMPTY，有车辆停靠 → HAS_VEHICLE，货物仍在购入车上 → ON_VEHICLE。" +
      "参数接受 id 或对象本身。",
  },
  {
    name: "log",
    kind: "method",
    signature: "log(...args)",
    template: "log(${args})",
    doc: "输出一行到日志面板。多参数以空格连接，非字符串参数 JSON 序列化。",
  },
  {
    name: "memory",
    kind: "property",
    signature: "memory → 受控映射",
    doc:
      "受控根映射：主进程持有的数据树，跨代码重载持久。" +
      "读写与嵌套修改即时提交；写入原生容器先深拷贝，之后改原变量不动 memory。" +
      "操作面两语言不同——JS：映射 keys()/size/to_dict()，列表 length/push/remove(i)/to_list()；" +
      "Python：映射 keys()/to_dict()/len(m)，列表 append/remove(i)/to_list()（remove 按下标）。" +
      "robots 键为宿主保留，玩家勿占用。",
  },
];
