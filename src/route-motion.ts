import { nextTick, onBeforeUnmount, watch, type VNodeRef } from "vue";

// Very light opacity-only fade on route change. Uses the native Web Animations API
// so the page element keeps its identity (KeepAlive routes are not remounted).
// A deeper fade or a position shift reads as a flash right after the pending route
// reveals, and a transform on the measured content element would offset native Tab
// bounds, so every page uses the same short, near-opaque fade.
export const ROUTE_MOTION_KEYFRAMES: Keyframe[] = [{ opacity: 0.96 }, { opacity: 1 }];
export const ROUTE_MOTION_TIMING: KeyframeAnimationOptions = { duration: 100, easing: "ease-out" };

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
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    animation = element.animate(ROUTE_MOTION_KEYFRAMES, ROUTE_MOTION_TIMING);
  }, { flush: "post" });

  onBeforeUnmount(() => { generation++; stop(); });
  const setElement: VNodeRef = (node) => { element = node instanceof HTMLElement ? node : null; };
  return setElement;
}
