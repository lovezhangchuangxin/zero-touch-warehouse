// 试玩示例脚本（与 records/b2-playthrough.md 记录对应）。
// 脚本本体放 demos/ 下，经 Vite ?raw 内联——同一份文本既是编辑器示例，
// 也是桌面端集成测试 include_str! 的对象（单一事实源）。
// A1 起示例分语言（js / py）；Python 示例与 JS 同地图可对照试玩。
// A2 起 JS 示例是 ESM 模块（export function loop 入口契约）；?raw 的
// 默认导出是 Vite 注入的整文件文本，import/default 规则解析不了，
// 对本文件的这组导入定向豁免。
/* oxlint-disable import/default */
import demoOne from "./demos/demo_one.js?raw";
import demoFiveLanes from "./demos/demo_five_lanes.js?raw";
import demoFiveNaive from "./demos/demo_five_naive.js?raw";
import demoMoveto from "./demos/demo_moveto.js?raw";
import demoPyBasics from "./demos/demo_py_basics.py?raw";
import demoTradeFast from "./demos/demo_trade_fast.js?raw";
import demoTradeHoard from "./demos/demo_trade_hoard.js?raw";
import demoTradeLeverage from "./demos/demo_trade_leverage.js?raw";
/* oxlint-enable import/default */

/** 玩家代码语言（与桌面端 Language 对应，"js" / "py"）。 */
export type Language = "js" | "py";

/** 语言下拉共用选项：编辑器语言切换与设置面板默认语言同源。 */
export const LANG_OPTIONS: { value: Language; label: string }[] = [
  { value: "js", label: "JavaScript" },
  { value: "py", label: "Python" },
];

export interface DemoScript {
  id: string;
  name: string;
  desc: string;
  language: Language;
  code: string;
}

export const DEMO_SCRIPTS: DemoScript[] = [
  {
    id: "demo-one",
    name: "单机全闭环",
    desc: "一台机器人跑完六张订单（约 80 tick）",
    language: "js",
    code: demoOne,
  },
  {
    id: "demo-five-naive",
    name: "五机贪心（堵塞）",
    desc: "五台机器人无分工抢道：看事件面板里的争抢码",
    language: "js",
    code: demoFiveNaive,
  },
  {
    id: "demo-five-lanes",
    name: "五机分流（对比）",
    desc: "认领分工 + 专属暂存格，同一地图吞吐显著改善",
    language: "js",
    code: demoFiveLanes,
  },
  {
    id: "demo-py-basics",
    name: "Python 入门（移动+记账）",
    desc: "与 JS 同地图对照：切换语言会重启宿主，Game.memory 保留",
    language: "py",
    code: demoPyBasics,
  },
  {
    id: "demo-moveto",
    name: "寻路巡逻（M4）",
    desc: "move_to 复合移动与 find_path 查询：四点巡逻，路径缓存跨热重载续走",
    language: "js",
    code: demoMoveto,
  },
  {
    id: "demo-trade-fast",
    name: "经营 · 快转（M3）",
    desc: "m3-trade 地图：只做矿泉水，紧阈值低买高卖、零库存目标",
    language: "js",
    code: demoTradeFast,
  },
  {
    id: "demo-trade-hoard",
    name: "经营 · 囤货（M3）",
    desc: "三货物低吸高抛，货架囤波段——资金沉淀在库存里",
    language: "js",
    code: demoTradeHoard,
  },
  {
    id: "demo-trade-leverage",
    name: "经营 · 杠杆囤货（M3）",
    desc: "囤货 + 借贷：现金枯竭时借款扫低价货，回款还债",
    language: "js",
    code: demoTradeLeverage,
  },
];
