import { mount } from "@vue/test-utils";
import { EditorView } from "@codemirror/view";
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

  it("keeps live-follow selection at the newest text on initial load, append, and rotation", async () => {
    const wrapper = mount(NvxCodeEditor, {
      props: {
        modelValue: "first\nsecond\n",
        followEnd: true,
        label: "Follow log",
      },
    });
    const view = EditorView.findFromDOM(wrapper.get(".cm-editor").element as HTMLElement);
    expect(view?.state.selection.main.head).toBe("first\nsecond\n".length);

    await wrapper.setProps({ modelValue: "first\nsecond\nthird\n" });
    expect(view?.state.selection.main.head).toBe("first\nsecond\nthird\n".length);

    await wrapper.setProps({ modelValue: "rotated\n" });
    expect(view?.state.selection.main.head).toBe("rotated\n".length);

    await wrapper.setProps({ followEnd: false, modelValue: "rotated\nmore\n" });
    await wrapper.setProps({ followEnd: true });
    expect(view?.state.selection.main.head).toBe("rotated\nmore\n".length);
    wrapper.unmount();
  });
});
