import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import { useTipsStore } from "../../stores/tips";
import NvxTips from "./NvxTips.vue";

describe("NvxTips", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("renders application feedback in the global top region and dismisses it", async () => {
    i18n.global.locale.value = "en";
    const pinia = createPinia();
    const wrapper = mount(NvxTips, {
      attachTo: document.body,
      global: { plugins: [pinia, i18n] },
    });
    const tips = useTipsStore(pinia);
    tips.show({ tone: "error", title: "Tunnel failed", durationMs: 0 });
    await wrapper.vm.$nextTick();

    const region = document.body.querySelector<HTMLElement>(".nvx-tips");
    const item = document.body.querySelector<HTMLElement>(".nvx-tips__item");
    expect(region?.getAttribute("aria-label")).toBe("Notifications");
    expect(region?.parentElement).toBe(document.body);
    expect(item?.getAttribute("role")).toBe("alert");
    expect(item?.textContent).toContain("Tunnel failed");

    document.body.querySelector<HTMLButtonElement>('.nvx-tips__dismiss')?.click();
    await wrapper.vm.$nextTick();
    expect(tips.items).toHaveLength(0);
    wrapper.unmount();
  });
});
