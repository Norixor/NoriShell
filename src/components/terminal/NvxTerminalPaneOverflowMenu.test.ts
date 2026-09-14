import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import NvxTerminalPaneOverflowMenu from "./NvxTerminalPaneOverflowMenu.vue";

describe("NvxTerminalPaneOverflowMenu", () => {
  it("groups narrow Pane actions behind one accessible menu button", async () => {
    const wrapper = mount(NvxTerminalPaneOverflowMenu, {
      props: {
        canSplitHorizontal: true,
        canSplitVertical: true,
        hasSelection: true,
        showLayoutActions: true,
        showSessionAction: true,
        sessionActionLabel: "断开",
        sessionActionDanger: true,
      },
      global: { plugins: [i18n] },
    });

    const trigger = wrapper.get('button[aria-label="更多终端操作"]');
    expect(trigger.attributes("aria-expanded")).toBe("false");
    await trigger.trigger("click");

    expect(trigger.attributes("aria-expanded")).toBe("true");
    expect(wrapper.get('[role="menu"]').isVisible()).toBe(true);
    expect(wrapper.findAll('[role="menuitem"]')).toHaveLength(6);

    await wrapper.get('button[role="menuitem"]:nth-of-type(5)').trigger("click");
    expect(wrapper.emitted("session")).toHaveLength(1);
    await trigger.trigger("click");
    await wrapper.get('button[role="menuitem"]:last-child').trigger("click");
    expect(wrapper.emitted("close")).toHaveLength(1);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
  });

  it("keeps copy disabled without a selection and closes on Escape", async () => {
    const wrapper = mount(NvxTerminalPaneOverflowMenu, {
      props: {
        canSplitHorizontal: true,
        canSplitVertical: true,
        hasSelection: false,
        showLayoutActions: false,
      },
      attachTo: document.body,
      global: { plugins: [i18n] },
    });

    await wrapper.get('button[aria-label="更多终端操作"]').trigger("click");
    expect(wrapper.get('button[role="menuitem"]:nth-child(2)').attributes()).toHaveProperty("disabled");

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });
});
