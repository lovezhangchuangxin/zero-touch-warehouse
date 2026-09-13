// 试玩示例脚本（与 records/b2-playthrough.md 记录对应）。
// 脚本本体放 demos/ 下的 .js 文件，经 Vite ?raw 内联——同一份文本
// 既是编辑器示例，也是桌面端集成测试 include_str! 的对象（单一事实源）。
import demoOne from "./demos/demo_one.js?raw";
import demoFiveLanes from "./demos/demo_five_lanes.js?raw";
import demoFiveNaive from "./demos/demo_five_naive.js?raw";

export interface DemoScript {
  id: string;
  name: string;
  desc: string;
  code: string;
}

export const DEMO_SCRIPTS: DemoScript[] = [
  {
    id: "demo-one",
    name: "单机全闭环",
    desc: "一台机器人跑完六张订单（约 80 tick）",
    code: demoOne,
  },
  {
    id: "demo-five-naive",
    name: "五机贪心（堵塞）",
    desc: "五台机器人无分工抢道：看事件面板里的争抢码",
    code: demoFiveNaive,
  },
  {
    id: "demo-five-lanes",
    name: "五机分流（对比）",
    desc: "认领分工 + 专属暂存格，同一地图吞吐显著改善",
    code: demoFiveLanes,
  },
];
