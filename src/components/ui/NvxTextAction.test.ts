import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxTextAction from "./NvxTextAction.vue";

describe("NvxTextAction", () => {
  it("emits click from a semantic text-only button", async () => {
    const wrapper = mount(NvxTextAction, {
      slots: { default: "Restore" },
    });
    const button = wrapper.get("button");

    expect(button.attributes("type")).toBe("button");
    expect(button.classes()).toContain("nvx-text-action--neutral");
    await button.trigger("click");
    expect(wrapper.emitted("click")).toHaveLength(1);
  });

  it("blocks disabled and loading interaction while preserving focus styling", async () => {
    const disabled = mount(NvxTextAction, {
      props: { disabled: true, tone: "danger" },
      slots: { default: "Restore" },
      attachTo: document.body,
    });
    const disabledButton = disabled.get<HTMLButtonElement>("button");
    expect(disabledButton.attributes("disabled")).toBeDefined();
    expect(disabledButton.classes()).toContain("nvx-text-action--danger");
    await disabledButton.trigger("click");
    expect(disabled.emitted("click")).toBeUndefined();

    const loading = mount(NvxTextAction, {
      props: { loading: true },
      slots: { default: "Restore" },
    });
    const loadingButton = loading.get("button");
    expect(loadingButton.attributes("aria-busy")).toBe("true");
    expect(loadingButton.attributes("disabled")).toBeDefined();
    expect(loadingButton.classes()).toContain("nvx-text-action--loading");
    expect(loading.text()).toContain("Restore");
    await loadingButton.trigger("click");
    expect(loading.emitted("click")).toBeUndefined();

    disabledButton.element.removeAttribute("disabled");
    disabledButton.element.focus();
    expect(document.activeElement).toBe(disabledButton.element);
    disabled.unmount();
  });
});
