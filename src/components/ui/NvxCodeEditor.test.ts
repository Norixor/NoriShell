import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxCodeEditor from "./NvxCodeEditor.vue";

describe("NvxCodeEditor", () => {
  it("updates the document and switches to a selectable read-only surface", async () => {
    const wrapper = mount(NvxCodeEditor, {
      props: {
        modelValue: "alpha",
        filename: "notes.txt",
        readonly: false,
        label: "Edit notes.txt",
      },
    });

    expect(wrapper.find(".cm-content").text()).toBe("alpha");
    await wrapper.setProps({ modelValue: "beta", readonly: true });

    expect(wrapper.find(".cm-content").text()).toBe("beta");
    expect(wrapper.find(".cm-content").attributes("contenteditable")).toBe("false");
    expect(wrapper.find(".cm-content").attributes("aria-label")).toBe("Edit notes.txt");
    wrapper.unmount();
  });
});
