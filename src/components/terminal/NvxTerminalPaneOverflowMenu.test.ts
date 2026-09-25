import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import NvxTerminalPaneOverflowMenu from "./NvxTerminalPaneOverflowMenu.vue";

describe("NvxTerminalPaneOverflowMenu", () => {
  beforeEach(() => { i18n.global.locale.value = "zh-CN"; });
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
      attachTo: document.body,
      global: { plugins: [i18n] },
    });

    const trigger = wrapper.get('button[aria-label="更多终端操作"]');
    expect(trigger.attributes("aria-expanded")).toBe("false");
    await trigger.trigger("click");

    expect(trigger.attributes("aria-expanded")).toBe("true");
    const menu = document.querySelector<HTMLElement>('.terminal-pane-overflow-menu__popover');
    expect(menu).not.toBeNull();
    expect(menu?.parentElement).toBe(document.body);
    expect(menu?.querySelectorAll('[role="menuitem"]')).toHaveLength(6);

    menu?.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]')[4]?.click();
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted("session")).toHaveLength(1);
    await trigger.trigger("click");
    document.querySelector<HTMLElement>('.terminal-pane-overflow-menu__popover')
      ?.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]')[5]?.click();
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted("close")).toHaveLength(1);
    expect(document.querySelector('.terminal-pane-overflow-menu__popover')).toBeNull();
    wrapper.unmount();
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
    expect(document.querySelector<HTMLButtonElement>('.terminal-pane-overflow-menu__popover button[role="menuitem"]:nth-child(2)')?.disabled).toBe(true);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await wrapper.vm.$nextTick();
    expect(document.querySelector('.terminal-pane-overflow-menu__popover')).toBeNull();
    wrapper.unmount();
  });
});
