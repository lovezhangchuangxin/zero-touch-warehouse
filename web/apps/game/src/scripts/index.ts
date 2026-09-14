// 试玩示例脚本（与 records/b2-playthrough.md 记录对应）。
// 脚本本体放 demos/ 下，经 Vite ?raw 内联——同一份文本既是编辑器示例，
// 也是桌面端集成测试 include_str! 的对象（单一事实源）。
// A1 起示例分语言（js / py）；Python 示例与 JS 同地图可对照试玩。
import demoOne from "./demos/demo_one.js?raw";
import demoFiveLanes from "./demos/demo_five_lanes.js?raw";
import demoFiveNaive from "./demos/demo_five_naive.js?raw";
import demoPyBasics from "./demos/demo_py_basics.py?raw";

/** 玩家代码语言（与桌面端 Language 对应，"js" / "py"）。 */
export type Language = "js" | "py";

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
];
