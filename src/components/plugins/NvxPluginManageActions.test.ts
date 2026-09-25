import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import NvxPluginManageActions from "./NvxPluginManageActions.vue";

function menu() {
  return document.querySelector<HTMLElement>('.plugin-manage-actions__popover');
}

function menuItems() {
  return Array.from(menu()?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
}

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
    expect(menu()?.parentElement).toBe(document.body);
    expect(menu()?.textContent).toContain("View package and capability details");
    expect(menu()?.textContent).toContain("Manage Permissions");
    expect(menu()?.textContent).toContain("Operation approvals");
    expect(menu()?.textContent).toContain("Uninstall");

    menuItems()[1]?.click();
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted("permissions")).toHaveLength(1);
    expect(menu()).toBeNull();
    wrapper.unmount();
  });

  it("opens remembered operation approvals from the existing plugin menu", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxPluginManageActions, {
      props: { state: "enabled" },
      global: { plugins: [i18n] },
    });
    await wrapper.get('button[aria-haspopup="menu"]').trigger("click");
    const action = menuItems().find((item) => item.textContent?.trim() === "Operation approvals");
    expect(action).toBeDefined();
    action?.click();
    await wrapper.vm.$nextTick();
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
    const settings = wrapper.findAll("button").find((item) => item.text().trim() === "Settings");
    expect(settings).toBeDefined();
    await settings?.trigger("click");
    expect(wrapper.emitted("settings")).toHaveLength(1);
    await wrapper.get('button[aria-haspopup="menu"]').trigger("click");
    expect(menuItems().some((item) => item.textContent?.trim() === "Settings")).toBe(false);
    await wrapper.setProps({ hasSettings: false });
    expect(wrapper.findAll("button").some((item) => item.text().trim() === "Settings")).toBe(false);
    wrapper.unmount();
  });

});
