import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { i18n } from "../../locales";
import NvxTerminalPluginRegion from "./NvxTerminalPluginRegion.vue";

const target = { props: ["disabled", "showIdentity"], emits: ["availability"], template: '<div>CPU 12%</div>' };
describe("terminal plugin status region", () => {
  it("keeps its target mounted with zero layout when empty and recovers when settings reveal it", async () => {
    const wrapper = mount(NvxTerminalPluginRegion, { props: { region: "footer", instanceKey: "ssh-a", contextLabel: "SSH", available: true },
      global: { plugins: [i18n], stubs: { NvxPluginExtensionTarget: target } } });
    const extension = wrapper.getComponent(target);
    expect(wrapper.attributes("style")).toContain("display: none");
    expect(wrapper.find("header").exists()).toBe(false);
    expect(extension.props("showIdentity")).toBe(false);
    extension.vm.$emit("availability", 1);
    await wrapper.vm.$nextTick();
    expect(wrapper.attributes("style") ?? "").not.toContain("display: none");
    extension.vm.$emit("availability", 0);
    await wrapper.vm.$nextTick();
    expect(wrapper.attributes("style")).toContain("display: none");
    expect(wrapper.findComponent(target).exists()).toBe(true);
    await wrapper.setProps({ available: false });
    expect(extension.props("disabled")).toBe(true);
    wrapper.unmount();
  });
});
