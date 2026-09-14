import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";

import NvxSelect from "../ui/NvxSelect.vue";
import { i18n } from "../../locales";

import NvxPluginApprovalPolicy from "./NvxPluginApprovalPolicy.vue";

describe("NvxPluginApprovalPolicy", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
  });

  it("offers an exact-operation approval and describes the selected risk", () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "once", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "remote" },
      global: { plugins: [i18n] },
    });
    const select = wrapper.getComponent(NvxSelect);
    expect(select.props("options")).toEqual([
      { value: "once", label: "Allow this time only" },
      { value: "always", label: "Always allow this exact operation", disabled: false },
    ]);
    expect(wrapper.text()).toContain("This operation can change the server or interrupt a service.");
  });

  it("disables always approval and resets a stale selection when remembering is unavailable", async () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "always", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "terminal" },
      global: { plugins: [i18n] },
    });
    await wrapper.setProps({ rememberPolicy: "unstableTarget" });
    expect(wrapper.emitted("update:modelValue")).toContainEqual(["once"]);
    const options = wrapper.getComponent(NvxSelect).props("options");
    if (!Array.isArray(options) || options[1] === undefined) throw new Error("missing always option");
    expect(options[1]).toMatchObject({ disabled: true });
    expect(wrapper.text()).toContain("The target can change before the action runs.");
    expect(wrapper.text()).toContain("It is not a safe command.");
  });

  it("shows expiry only for remembered decisions and forwards the selected duration", async () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "once", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "file" },
      global: { plugins: [i18n] },
    });
    expect(wrapper.findAllComponents(NvxSelect)).toHaveLength(1);
    await wrapper.setProps({ modelValue: "always" });
    const expiry = wrapper.findAllComponents(NvxSelect)[1];
    if (!expiry) throw new Error("missing expiry selector");
    expect(expiry.props("modelValue")).toBe("unlimited");
    expiry.vm.$emit("update:modelValue", "fifteenMinutes");
    expect(wrapper.emitted("update:modelExpiry")).toEqual([["fifteenMinutes"]]);
    await wrapper.setProps({ rememberPolicy: "unstableTarget" });
    expect(wrapper.findAllComponents(NvxSelect)).toHaveLength(1);
  });
});
