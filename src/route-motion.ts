import { nextTick, onBeforeUnmount, watch, type VNodeRef } from "vue";

// Short fade on route change. Uses the native Web Animations API so the page
// element keeps its identity (KeepAlive routes are not remounted).
export function useRouteMotion(routeKey: () => string | null) {
  let element: HTMLElement | null = null;
  let animation: Animation | null = null;
  let generation = 0;

  function stop() {
    animation?.cancel();
    animation = null;
  }

  watch(routeKey, async (key) => {
    const current = ++generation;
    stop();
    if (key === null) return;
    await nextTick();
    if (current !== generation || !element || typeof element.animate !== "function") return;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const isTerminal = key === "/terminal" || key === "/desktop" || key === "/sftp";
    const opacity = [reduced ? 0.96 : 0.86, 1];
    const keyframes = reduced || isTerminal
      ? { opacity }
      : { opacity, transform: ["translateY(5px)", "translateY(0)"] };
    animation = element.animate(keyframes, { duration: reduced ? 100 : 180, easing: "ease-out" });
  }, { flush: "post" });

  onBeforeUnmount(() => { generation++; stop(); });
  const setElement: VNodeRef = (node) => { element = node instanceof HTMLElement ? node : null; };
  return setElement;
}
