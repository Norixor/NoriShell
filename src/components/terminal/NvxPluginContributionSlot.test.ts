import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { nextTick, ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { usePluginsStore } from "../../stores/plugins";
import NvxPluginExtensionTarget from "../plugins/NvxPluginExtensionTarget.vue";
import NvxPluginContributionSlot from "./NvxPluginContributionSlot.vue";
import { terminalPluginToolsKey } from "./terminalPluginTools";

describe("NvxPluginContributionSlot", () => {
  beforeEach(() => { i18n.global.locale.value = "zh-CN"; });

  it("hides empty terminal plugin controls and shows them when this Pane has tools", async () => {
    const pinia = createPinia();
    vi.spyOn(usePluginsStore(pinia), "loadContributions").mockResolvedValue(undefined);
    const available = ref(false);
    const open = vi.fn();
    const wrapper = mount(NvxPluginContributionSlot, {
      props: { extensionSlot: "terminalToolbar", toolbarMenu: true, instanceKey: "pane-a" },
      global: {
        plugins: [pinia, i18n],
        provide: { [terminalPluginToolsKey]: { open, available } },
      },
    });
    await flushPromises();

    expect(wrapper.get(".plugin-slot").attributes("style")).toContain("display: none");
    available.value = true;
    await nextTick();
    expect(wrapper.get(".plugin-slot").attributes("style") ?? "").not.toContain("display: none");
    await wrapper.get('button[aria-label="打开插件工具"]').trigger("click");
    expect(open).toHaveBeenCalledWith("pane-a");
    available.value = false;
    await nextTick();
    expect(wrapper.get(".plugin-slot").attributes("style")).toContain("display: none");
    wrapper.unmount();
  });

  it("hides the narrow menu entry until a terminal contribution is available", async () => {
    const pinia = createPinia();
    vi.spyOn(usePluginsStore(pinia), "loadContributions").mockResolvedValue(undefined);
    const available = ref(false);
    const wrapper = mount(NvxPluginContributionSlot, {
      props: { extensionSlot: "terminalToolbar", menu: true, instanceKey: "pane-a" },
      global: {
        plugins: [pinia, i18n],
        provide: { [terminalPluginToolsKey]: { open: vi.fn(), available } },
      },
    });
    await flushPromises();

    expect(wrapper.get(".plugin-slot").attributes("style")).toContain("display: none");
    wrapper.findComponent(NvxPluginExtensionTarget).vm.$emit("availability", 1);
    await nextTick();
    expect(wrapper.get(".plugin-slot").attributes("style") ?? "").not.toContain("display: none");
    wrapper.unmount();
  });
});
