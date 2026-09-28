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

  it("uses a short opacity-only transition when motion is reduced", async () => {
    stubReducedMotion(true);
    const route = ref<string | null>(null);
    const { wrapper, animate } = mountHost(route);
    route.value = "/hosts";
    await flushPromises();
    expect(animate).toHaveBeenCalledWith({ opacity: [0.96, 1] }, { duration: 100, easing: "ease-out" });
    wrapper.unmount();
  });

  it("retains a gentle position transition for ordinary pages", async () => {
    stubReducedMotion(false);
    const route = ref<string | null>(null);
    const { wrapper, animate } = mountHost(route);
    route.value = "/hosts";
    await flushPromises();
    expect(animate).toHaveBeenCalledWith(
      { opacity: [0.86, 1], transform: ["translateY(5px)", "translateY(0)"] },
      { duration: 180, easing: "ease-out" },
    );
    wrapper.unmount();
  });
});
