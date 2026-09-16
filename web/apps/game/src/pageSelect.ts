// 拖拽类交互（Splitter 分栏 / Stage 画布平移）共用的页面级文本选择抑制：
// WebKit 的拖拽选择不受指针捕获约束，还会跳过无可选文本的按下点、把选择
// 锚落到文档序最近的可选内容（抽屉 / 编辑器），内容随轮询刷新时扫出的
// 选区被反复重画——表现为拖拽时页面文本整片闪烁选中。
// 配合调用方在 pointerdown 上的 preventDefault（阻止兼容 mousedown，选择
// 无从发起）。返回释放函数；重复调用安全。

export function suppressPageSelect(): () => void {
  const block = (e: Event): void => e.preventDefault();
  document.body.style.setProperty("user-select", "none");
  document.body.style.setProperty("-webkit-user-select", "none");
  document.addEventListener("selectstart", block, true);
  return () => {
    document.body.style.setProperty("user-select", "");
    document.body.style.setProperty("-webkit-user-select", "");
    document.removeEventListener("selectstart", block, true);
  };
}
