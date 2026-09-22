import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../../locales";
import NvxTerminalPaneControls from "./NvxTerminalPaneControls.vue";

describe("NvxTerminalPaneControls", () => {
  beforeEach(() => { i18n.global.locale.value = "zh-CN"; });
  it("keeps split, session, and close actions in distinct toolbar groups", async () => {
    const wrapper = mount(NvxTerminalPaneControls, {
      props: {
        canSplitHorizontal: true,
        canSplitVertical: true,
        showLayoutActions: true,
      },
      slots: { default: "<button type=\"button\">终止</button>" },
      global: { plugins: [i18n] },
    });

    expect(wrapper.findAll(".terminal-pane-controls__separator")).toHaveLength(2);
    await wrapper.get('button[aria-label="向右拆分 Pane"]').trigger("click");
    await wrapper.get('button[aria-label="向下拆分 Pane"]').trigger("click");
    await wrapper.get('button[aria-label="关闭当前 Pane"]').trigger("click");

    expect(wrapper.emitted("split")).toEqual([["horizontal"], ["vertical"]]);
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("keeps only the session action on an inactive Pane", () => {
    const wrapper = mount(NvxTerminalPaneControls, {
      props: {
        canSplitHorizontal: true,
        canSplitVertical: true,
        showLayoutActions: false,
      },
      slots: { default: "<button type=\"button\">终止</button>" },
      global: { plugins: [i18n] },
    });

    expect(wrapper.text()).toContain("终止");
    expect(wrapper.findAll(".terminal-pane-controls__separator")).toHaveLength(0);
    expect(wrapper.find('button[aria-label="向右拆分 Pane"]').exists()).toBe(false);
    expect(wrapper.find('button[aria-label="关闭当前 Pane"]').exists()).toBe(false);
  });
});
