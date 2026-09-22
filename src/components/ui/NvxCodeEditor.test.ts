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

  it("searches the document through the external file toolbar API", () => {
    const wrapper = mount(NvxCodeEditor, {
      props: {
        modelValue: "alpha beta alpha",
        filename: "notes.txt",
        label: "Edit notes.txt",
      },
    });
    const editor = wrapper.vm as unknown as {
      updateSearch(query: string): boolean;
      findNext(): boolean;
      findPrevious(): boolean;
    };

    expect(editor.updateSearch("alpha")).toBe(true);
    expect(editor.findNext()).toBe(true);
    expect(editor.findPrevious()).toBe(true);
    expect(editor.updateSearch("missing")).toBe(false);
    expect(editor.updateSearch("")).toBe(true);
    wrapper.unmount();
  });
});
