import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";

import NvxSelect from "../ui/NvxSelect.vue";
import { i18n } from "../../locales";

import NvxPluginApprovalPolicy from "./NvxPluginApprovalPolicy.vue";

describe("NvxPluginApprovalPolicy", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
  });

  it("offers a remembered operation approval and describes the selected risk", () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "once", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "remote" },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("This operation can change the server or interrupt a service.");
    expect(wrapper.find("input[type=radio]").exists()).toBe(false);
    wrapper.unmount();

    const choice = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "once", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "remote", part: "choice" },
      global: { plugins: [i18n] },
    });
    expect(choice.findAll("input[type=radio]")).toHaveLength(2);
    expect(choice.text()).toContain("Allow this time only");
    expect(choice.text()).toContain("Remember this operation");
    expect(choice.get('input[value="once"]').attributes("checked")).toBeDefined();
  });

  it("disables always approval and resets a stale selection when remembering is unavailable", async () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "always", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "terminal", part: "choice" },
      global: { plugins: [i18n] },
    });
    await wrapper.setProps({ rememberPolicy: "unstableTarget" });
    expect(wrapper.emitted("update:modelValue")).toContainEqual(["once"]);
    expect(wrapper.get('input[value="always"]').attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("The target can change before the action runs.");
  });

  it("shows expiry only for remembered decisions and forwards the selected duration", async () => {
    const wrapper = mount(NvxPluginApprovalPolicy, {
      props: { modelValue: "once", modelExpiry: "unlimited", rememberPolicy: "exactOperation", risk: "file", part: "choice" },
      global: { plugins: [i18n] },
    });
    expect(wrapper.findAllComponents(NvxSelect)).toHaveLength(0);
    await wrapper.setProps({ modelValue: "always" });
    const expiry = wrapper.findAllComponents(NvxSelect)[0];
    if (!expiry) throw new Error("missing expiry selector");
    expect(expiry.props("modelValue")).toBe("unlimited");
    expiry.vm.$emit("update:modelValue", "fifteenMinutes");
    expect(wrapper.emitted("update:modelExpiry")).toEqual([["fifteenMinutes"]]);
    await wrapper.setProps({ rememberPolicy: "unstableTarget" });
    expect(wrapper.findAllComponents(NvxSelect)).toHaveLength(0);
  });
});
