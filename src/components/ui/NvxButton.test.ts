import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxButton from "./NvxButton.vue";

describe("NvxButton", () => {
  it("preserves content width and blocks interaction while loading", async () => {
    const wrapper = mount(NvxButton, {
      props: { loading: true },
      slots: { default: "Connect" },
    });
    const button = wrapper.get("button");
    expect(button.attributes("aria-busy")).toBe("true");
    expect(button.attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("Connect");
    await button.trigger("click");
    expect(wrapper.emitted("click")).toBeUndefined();
  });

  it("can keep a visible operation label beside the loading indicator", () => {
    const wrapper = mount(NvxButton, {
      props: { loading: true, loadingLabel: "Unlocking Vault…" },
      slots: { default: "Unlock Vault" },
    });

    expect(wrapper.get("button").attributes("aria-label")).toBe("Unlocking Vault…");
    expect(wrapper.get(".nvx-button__loader").text()).toBe("Unlocking Vault…");
  });
});
