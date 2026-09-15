// 滚动渐隐指令（v-fade）：滚动条已全局隐藏，容器溢出时在上下边缘
// 叠加向 panel 底色过渡的渐变条，作为「还有内容」的唯一可见线索。
// 实现为容器内的 sticky 零高哨兵：随滚动吸附在滚动口边缘，不参与布局。
// 内容增删改变的是 scrollHeight 而非容器盒尺寸，故除 ResizeObserver
// 外还需 MutationObserver 才能覆盖流式日志这类最常见路径。

import type { Directive } from "vue";

interface FadeState {
  update(): void;
  onScroll(): void;
  ro: ResizeObserver;
  mo: MutationObserver;
}

export const vFade: Directive<HTMLElement> = {
  mounted(el) {
    // 渐变色终结于面板底色：所有滚动区都坐在 bg-panel 之上。
    const make = (atBottom: boolean) => {
      const sentinel = document.createElement("div");
      sentinel.style.cssText = `position:sticky;${atBottom ? "bottom" : "top"}:0;left:0;height:0;z-index:1;pointer-events:none`;
      const bar = document.createElement("div");
      bar.style.cssText = `height:14px;margin-${atBottom ? "top" : "bottom"}:-14px;background:linear-gradient(to ${atBottom ? "top" : "bottom"}, var(--color-panel), transparent);opacity:0;transition:opacity 0.15s`;
      sentinel.appendChild(bar);
      // 顶部哨兵必须是首子节点（贴滚动口顶边），底部哨兵是末子节点；
      // grid / flex 容器会把哨兵当布局项，此类容器应把 v-fade 放在
      // 外层 block 包装上。
      if (atBottom) {
        el.appendChild(sentinel);
      } else {
        el.insertBefore(sentinel, el.firstChild);
      }
      return bar;
    };
    const top = make(false);
    const bottom = make(true);
    const update = () => {
      top.style.opacity = el.scrollTop > 2 ? "1" : "0";
      bottom.style.opacity = el.scrollTop + el.clientHeight < el.scrollHeight - 2 ? "1" : "0";
    };
    const state: FadeState = {
      update,
      onScroll: update,
      ro: new ResizeObserver(update),
      // update 只写 style.opacity，不会反过来触发自身（无回环）。
      mo: new MutationObserver(update),
    };
    state.ro.observe(el);
    state.mo.observe(el, { childList: true, subtree: true, characterData: true });
    el.addEventListener("scroll", state.onScroll, { passive: true });
    (el as HTMLElement & { __ztwFade?: FadeState }).__ztwFade = state;
    update();
  },
  unmounted(el) {
    const state = (el as HTMLElement & { __ztwFade?: FadeState }).__ztwFade;
    if (state) {
      state.ro.disconnect();
      state.mo.disconnect();
      el.removeEventListener("scroll", state.onScroll);
    }
  },
};
