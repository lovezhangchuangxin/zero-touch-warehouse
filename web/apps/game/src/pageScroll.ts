// 游戏外壳的输入守卫：掐掉浏览器默认的「网页感」。过度滚动（根层橡皮筋
// 与面板滚动链接）由 CSS overscroll-behavior 承担（main.css），这里拦
// CSS 够不到的缩放路径：触控板捏合在两大引擎都会合成 ctrl+wheel，默认
// 行为是缩放页面——画布区域由 Stage 自行消费该事件做画布缩放（目标阶段
// 已 preventDefault），本守卫挂 window 捕获其冒泡，只影响画布之外的
// 区域；不拦 WebKit 原生 gesture* 事件，否则可能掐断画布捏合的事件合成。

export function installScrollGuards(): void {
  window.addEventListener(
    "wheel",
    (e) => {
      if (e.ctrlKey) e.preventDefault();
    },
    { passive: false },
  );
}
