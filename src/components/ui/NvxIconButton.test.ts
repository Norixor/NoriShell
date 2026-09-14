import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxIconButton from "./NvxIconButton.vue";

describe("NvxIconButton", () => {
  it("requires and exposes an accessible name", () => {
    const wrapper = mount(NvxIconButton, {
      props: { label: "Close window" },
      slots: { default: "×" },
    });

    expect(wrapper.get("button").attributes("aria-label")).toBe("Close window");
    expect(wrapper.get("button").attributes("title")).toBe("Close window");
  });
});
