import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { defineComponent } from "vue";
import { en } from "../../locales/messages/en";
import NvxSftpFileWindow from "./NvxSftpFileWindow.vue";
import type { ToolTarget } from "../../tool-windows";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const target = { kind: "sftpFile", title: "Server · config.txt", tail: false } as Extract<ToolTarget, { kind: "sftpFile" }>;
const editorSearch = {
  focus: vi.fn(),
  updateSearch: vi.fn(() => true),
  findNext: vi.fn(() => true),
  findPrevious: vi.fn(() => true),
};
const CodeEditorStub = defineComponent({
  props: {
    modelValue: { type: String, required: true },
    readonly: { type: Boolean, default: false },
  },
  emits: ["update:modelValue"],
  setup(_, { expose }) { expose(editorSearch); },
  template: '<textarea :value="modelValue" :readonly="readonly" @input="$emit(\'update:modelValue\', $event.target.value)" />',
});
function setup(tail = false) {
  return mount(NvxSftpFileWindow, { props: { target: { ...target, tail } }, global: {
    plugins: [createI18n({ legacy: false, locale: "en", messages: { en } })],
    stubs: { NvxCodeEditor: CodeEditorStub },
  } });
}
beforeEach(() => {
  vi.clearAllMocks();
  editorSearch.updateSearch.mockReturnValue(true);
  editorSearch.findNext.mockReturnValue(true);
  editorSearch.findPrevious.mockReturnValue(true);
  vi.mocked(invoke).mockResolvedValue({ content: { kind: "text", text: "original\r\n", editable: true, truncated: false, lineEnding: "crLf", endOffset: "10" } });
});
afterEach(() => { document.body.innerHTML = ""; vi.useRealTimers(); });
describe("SFTP independent file window", () => {
  it("keeps a dirty draft until explicit discard and sends no session-close command", async () => {
    const wrapper = setup(); await flushPromises();
    await wrapper.get("textarea").setValue("changed\n");
    const closing = wrapper.vm.requestClose(); await flushPromises();
    const buttons = [...document.querySelectorAll("button")];
    buttons.find((button) => button.textContent?.includes("Keep editing"))?.click(); await flushPromises();
    expect(await closing).toBe(false);
    expect(wrapper.get("textarea").element.value).toBe("changed\n");
    wrapper.unmount();
    expect(vi.mocked(invoke).mock.calls.every(([command]) => command === "tool_file_preview")).toBe(true);
  });
  it("saves using the Core-bound file and preserves line endings", async () => {
    const wrapper = setup(); await flushPromises();
    await wrapper.get("textarea").setValue("changed\nsecond\n");
    await wrapper.findAll("button").find((button) => button.text().includes("Save"))!.trigger("click"); await flushPromises();
    expect(invoke).toHaveBeenCalledWith("tool_file_save", { text: "changed\r\nsecond\r\n", meta: expect.any(Object), operationId: expect.any(String) });
    expect(wrapper.emitted("saved")).toHaveLength(1);
    wrapper.unmount();
  });
  it("retains dirty content after save rejection", async () => {
    const wrapper = setup(); await flushPromises();
    await wrapper.get("textarea").setValue("unsaved");
    vi.mocked(invoke).mockRejectedValueOnce(new Error("stale"));
    await wrapper.findAll("button").find((button) => button.text().includes("Save"))!.trigger("click"); await flushPromises();
    expect(wrapper.emitted("saved")).toBeUndefined();
    expect(wrapper.get("textarea").element.value).toBe("unsaved");
    wrapper.unmount();
  });
  it("does not resume polling after the file window is destroyed", async () => {
    vi.useFakeTimers();
    let complete!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async (command) => command === "tool_file_preview"
      ? { content: { kind: "text", text: "", editable: true, truncated: false, lineEnding: "lf", endOffset: "0" } }
      : new Promise((resolve) => { complete = resolve; }));
    const wrapper = setup(true); await flushPromises();
    expect(invoke).toHaveBeenCalledWith("tool_file_tail", { offset: "0" });
    wrapper.unmount(); complete({ bytes: [65], reset: false, nextOffset: "1", totalSize: "1" }); await flushPromises();
    await vi.advanceTimersByTimeAsync(3000);
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("opens the file search toolbar and delegates navigation to the editor", async () => {
    const wrapper = setup(); await flushPromises();
    await wrapper.get('button[aria-label="Search in file"]').trigger("click");
    const search = wrapper.get('input[aria-label="Search in file"]');
    await search.setValue("original");
    expect(editorSearch.updateSearch).toHaveBeenCalledWith("original");
    await wrapper.get('button[aria-label="Next match"]').trigger("click");
    expect(editorSearch.findNext).toHaveBeenCalledOnce();
    await wrapper.get('button[aria-label="Close search"]').trigger("click");
    expect(editorSearch.updateSearch).toHaveBeenLastCalledWith("");
  });
});
