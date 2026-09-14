import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxPluginCapabilityChoice from "./NvxPluginCapabilityChoice.vue";

describe("protected capability choice", () => {
  it("opens approval without changing the permission when its input or label is clicked", async () => {
    const wrapper = mount(NvxPluginCapabilityChoice, {
      props: { modelValue: false, requiresApproval: true },
      slots: { default: "Inspect server" },
    });
    const input = wrapper.get("input");
    expect(input.attributes("disabled")).toBeUndefined();
    await input.trigger("click");
    await wrapper.get("label").trigger("click");
    expect(wrapper.emitted("requestApproval")).toHaveLength(2);
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();
    expect((input.element as HTMLInputElement).checked).toBe(false);
    wrapper.unmount();
  });

  it("blocks repeated approval while pending and preserves ordinary checkbox changes", async () => {
    const wrapper = mount(NvxPluginCapabilityChoice, {
      props: { modelValue: false, requiresApproval: true, disabled: true },
    });
    await wrapper.get("label").trigger("click");
    expect(wrapper.emitted("requestApproval")).toBeUndefined();
    await wrapper.setProps({ requiresApproval: false, disabled: false });
    await wrapper.get("input").setValue(true);
    expect(wrapper.emitted("update:modelValue")).toEqual([[true]]);
    wrapper.unmount();
  });
});
