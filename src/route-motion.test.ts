import { mount, flushPromises } from "@vue/test-utils";
import { defineComponent, h, ref } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useRouteMotion } from "./route-motion";

function mountHost(route: ReturnType<typeof ref<string | null>>) {
  const animate = vi.fn(() => ({ cancel: vi.fn() }));
  const Host = defineComponent({
    setup() {
      const setElement = useRouteMotion(() => route.value ?? null);
      return () => h("div", { ref: setElement });
    },
  });
  const wrapper = mount(Host);
  (wrapper.element as HTMLElement).animate = animate as unknown as HTMLElement["animate"];
  return { wrapper, animate };
}

function stubReducedMotion(reduced: boolean) {
  vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: reduced })));
}

describe("route motion", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("does not animate page changes when motion is reduced", async () => {
    stubReducedMotion(true);
    const route = ref<string | null>(null);
    const { wrapper, animate } = mountHost(route);
    route.value = "/hosts";
    await flushPromises();
    expect(animate).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses one near-opaque, opacity-only fade without a position shift", async () => {
    stubReducedMotion(false);
    const route = ref<string | null>(null);
    const { wrapper, animate } = mountHost(route);
    for (const path of ["/hosts", "/terminal", "/settings"]) {
      route.value = path;
      await flushPromises();
    }
    expect(animate).toHaveBeenCalledTimes(3);
    for (const [keyframes, timing] of animate.mock.calls as unknown as [Keyframe[], KeyframeAnimationOptions][]) {
      expect(keyframes).toEqual([{ opacity: 0.96 }, { opacity: 1 }]);
      expect(keyframes.some((frame) => "transform" in frame)).toBe(false);
      expect(timing).toEqual({ duration: 100, easing: "ease-out" });
    }
    wrapper.unmount();
  });

  it("does not animate while the route is pending", async () => {
    stubReducedMotion(false);
    const route = ref<string | null>("/hosts");
    const { wrapper, animate } = mountHost(route);
    route.value = null;
    await flushPromises();
    expect(animate).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
