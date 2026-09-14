import { mount, flushPromises } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NvxHighlightSettings from "./NvxHighlightSettings.vue";
import { NvxButton, NvxCheckbox, NvxInput, NvxSelect } from "../ui";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { highlightingEn } from "../../locales/highlighting";

class PreviewWorker {
  static instances: PreviewWorker[] = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  postMessage = vi.fn();
  terminate = vi.fn();
  constructor() { PreviewWorker.instances.push(this); }
}
function setup() {
  const pinia = createPinia();
  const wrapper = mount(NvxHighlightSettings, { props: { hosts: [{ hostId: "host-a", label: "Production" }] }, global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { highlighting: highlightingEn } } })], stubs: { teleport: true } } });
  return { wrapper, store: useTerminalPreferencesStore(pinia), tips: useTipsStore(pinia) };
}
type Wrapper = ReturnType<typeof setup>["wrapper"];
function button(wrapper: Wrapper, text: string) { return wrapper.findAllComponents(NvxButton).find(item => item.text() === text)!; }
function input(wrapper: Wrapper, label: string) { return wrapper.findAllComponents(NvxInput).find(item => item.props("ariaLabel") === label)!; }
async function openFirstRule(wrapper: Wrapper) { await button(wrapper, "Edit").trigger("click"); }

describe("highlight settings", () => {
  beforeEach(() => { localStorage.clear(); PreviewWorker.instances = []; vi.stubGlobal("Worker", PreviewWorker); });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
  it("keeps edits local, cancels, and persists only when saving", async () => {
    const { wrapper, store } = setup();
    wrapper.findComponent(NvxCheckbox).vm.$emit("update:modelValue", true);
    await flushPromises();
    expect(store.preferences.highlights.enabled).toBe(false);
    await button(wrapper, "Cancel changes").trigger("click");
    expect(wrapper.findComponent(NvxCheckbox).props("modelValue")).toBe(false);
    wrapper.findComponent(NvxCheckbox).vm.$emit("update:modelValue", true);
    await flushPromises();
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.highlights.enabled).toBe(true);
    wrapper.unmount();
  });
  it("preserves the draft and old preferences when storage fails", async () => {
    const { wrapper, store, tips } = setup();
    wrapper.findComponent(NvxCheckbox).vm.$emit("update:modelValue", true);
    await flushPromises();
    const spy = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.highlights.enabled).toBe(false);
    expect(wrapper.findComponent(NvxCheckbox).props("modelValue")).toBe(true);
    expect(tips.items[0]?.title).toBe(highlightingEn.saveFailed);
    spy.mockRestore(); tips.clearAll(); wrapper.unmount();
  });
  it("supports host inheritance, disabling, and custom rules", async () => {
    const { wrapper, store } = setup();
    wrapper.findComponent(NvxSelect).vm.$emit("update:modelValue", "host-a");
    await flushPromises();
    const select = wrapper.findAllComponents(NvxSelect).find(item => item.props("ariaLabel") === "Host behavior")!;
    expect(select.props("modelValue")).toBe("inherit");
    select.vm.$emit("update:modelValue", "disabled");
    await flushPromises();
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.hostHighlights["host-a"]?.mode).toBe("disabled");
    select.vm.$emit("update:modelValue", "custom");
    await flushPromises();
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.hostHighlights["host-a"]?.mode).toBe("custom");
    select.vm.$emit("update:modelValue", "inherit");
    await flushPromises();
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.hostHighlights["host-a"]).toBeUndefined();
    wrapper.unmount();
  });
  it("validates rules, warns on contrast, and cancels modal drafts", async () => {
    const { wrapper, store } = setup();
    await openFirstRule(wrapper);
    input(wrapper, "Text or pattern").vm.$emit("update:modelValue", "(");
    input(wrapper, "Text color").vm.$emit("update:modelValue", "#000000");
    input(wrapper, "Background color").vm.$emit("update:modelValue", "#000000");
    await flushPromises();
    expect(button(wrapper, "Keep rule").props("disabled")).toBe(true);
    expect(wrapper.text()).toContain("Text contrast is 1.00:1");
    await button(wrapper, "Cancel").trigger("click");
    expect(store.preferences.highlights.rules[0]?.pattern).toContain("ERROR");
    expect(button(wrapper, "Save changes").props("disabled")).toBe(true);
    wrapper.unmount();
  });
  it("previews exclusively via Worker and rejects stale responses after editing", async () => {
    const { wrapper } = setup();
    await openFirstRule(wrapper);
    await button(wrapper, "Test preview").trigger("click");
    const worker = PreviewWorker.instances[0]!;
    const request = worker.postMessage.mock.calls[0]![0];
    expect(request.lines[0]).toContain("连接失败");
    expect(request.rules[0].pattern).toContain("ERROR");
    input(wrapper, "Text or pattern").vm.$emit("update:modelValue", "FATAL");
    await flushPromises();
    expect(worker.terminate).toHaveBeenCalled();
    worker.onmessage?.({ data: { requestId: request.requestId, matches: [{ line: 0, start: 11, end: 16, ruleId: "error" }] } } as MessageEvent);
    await flushPromises();
    expect(wrapper.text()).toContain(highlightingEn.previewIdle);
    expect(wrapper.findAll(".nvx-highlight-settings__line span").some(item => item.attributes("style")?.includes("background"))).toBe(false);
    wrapper.unmount();
  });
  it("renders bounded Worker matches as text segments and terminates the worker", async () => {
    const { wrapper } = setup();
    await openFirstRule(wrapper);
    await button(wrapper, "Test preview").trigger("click");
    const worker = PreviewWorker.instances[0]!;
    const request = worker.postMessage.mock.calls[0]![0];
    worker.onmessage?.({ data: { requestId: request.requestId, matches: [
      { line: 0, start: 11, end: 16, ruleId: "error" },
      { line: 100, start: 0, end: 1, ruleId: "error" },
    ] } } as MessageEvent);
    await flushPromises();
    expect(wrapper.text()).toContain("1 matches in sample text");
    const marked = wrapper.findAll(".nvx-highlight-settings__line span").filter(item => item.attributes("style")?.includes("background"));
    expect(marked.map(item => item.text())).toEqual(["ERROR"]);
    expect(worker.terminate).toHaveBeenCalled();
    wrapper.unmount();
  });
  it("terminates timed-out previews and allows valid rules without a Worker", async () => {
    vi.useFakeTimers();
    const { wrapper } = setup();
    await openFirstRule(wrapper);
    await button(wrapper, "Test preview").trigger("click");
    await vi.advanceTimersByTimeAsync(501);
    expect(PreviewWorker.instances[0]!.terminate).toHaveBeenCalled();
    expect(wrapper.text()).toContain(highlightingEn.previewTimeout);
    vi.stubGlobal("Worker", undefined);
    await button(wrapper, "Test preview").trigger("click");
    expect(wrapper.text()).toContain(highlightingEn.previewUnavailable);
    expect(button(wrapper, "Keep rule").props("disabled")).toBe(false);
    wrapper.unmount();
  });
});
