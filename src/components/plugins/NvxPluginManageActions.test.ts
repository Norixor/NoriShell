import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import NvxPluginManageActions from "./NvxPluginManageActions.vue";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("NvxPluginManageActions", () => {
  it("keeps the lifecycle action visible and moves secondary actions into a keyboard menu", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      attachTo: document.body,
      props: { state: "enabled" },
      global: { plugins: [i18n] },
    });

    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("disable")).toHaveLength(1);

    await wrapper.get('button[aria-label="More plugin actions"]').trigger("click");
    expect(wrapper.get('[role="menu"]').text()).toContain("View package and capability details");
    expect(wrapper.get('[role="menu"]').text()).toContain("Manage Permissions");
    expect(wrapper.get('[role="menu"]').text()).toContain("Operation approvals");
    expect(wrapper.get('[role="menu"]').text()).toContain("Uninstall");

    const permissions = wrapper.findAll('[role="menuitem"]')[1];
    await permissions?.trigger("click");
    expect(wrapper.emitted("permissions")).toHaveLength(1);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("opens remembered operation approvals from the existing plugin menu", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      props: { state: "enabled" },
      global: { plugins: [i18n] },
    });
    await wrapper.get('button[aria-haspopup="menu"]').trigger("click");
    const action = wrapper.findAll('[role="menuitem"]').find((item) => item.text() === "Operation approvals");
    expect(action).toBeDefined();
    await action!.trigger("click");
    expect(wrapper.emitted("operationPermissions")).toHaveLength(1);
    wrapper.unmount();
  });

  it("offers enable for a disabled plugin", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      props: { state: "disabled" },
      global: { plugins: [i18n] },
    });

    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("enable")).toHaveLength(1);
  });

  it("lets a crashed plugin restart without an online update action", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      props: { state: "crashed" },
      global: { plugins: [i18n] },
    });

    const buttons = wrapper.findAll("button");
    expect(buttons[0]?.text()).toContain("Enable");
    expect(wrapper.text()).not.toContain("Update");
    await buttons[0]?.trigger("click");
    expect(wrapper.emitted("enable")).toHaveLength(1);
  });
  it("offers settings only for a declared schema, including disabled plugins", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      props: { state: "disabled", hasSettings: true },
      global: { plugins: [i18n] },
    });
    await wrapper.get('button[aria-haspopup="menu"]').trigger("click");
    const settings = wrapper.findAll('[role="menuitem"]').find((item) => item.text() === "Settings");
    expect(settings).toBeDefined();
    await settings!.trigger("click");
    expect(wrapper.emitted("settings")).toHaveLength(1);
    await wrapper.setProps({ hasSettings: false });
    await wrapper.get('button[aria-haspopup="menu"]').trigger("click");
    expect(wrapper.findAll('[role="menuitem"]').some((item) => item.text() === "Settings")).toBe(false);
    wrapper.unmount();
  });

});
