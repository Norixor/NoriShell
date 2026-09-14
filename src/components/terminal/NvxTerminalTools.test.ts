import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import NvxTerminalTools from "./NvxTerminalTools.vue";

const target = {
  findNext: vi.fn(),
  findPrevious: vi.fn(),
  clearSearch: vi.fn(),
  selection: vi.fn(),
  focus: vi.fn(),
};

describe("NvxTerminalTools", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    target.findNext.mockReturnValue(true);
    target.findPrevious.mockReturnValue(true);
    target.selection.mockReturnValue("selected output");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("searches incrementally, forward, and backward, then restores terminal focus", async () => {
    const wrapper = mount(NvxTerminalTools, {
      attachTo: document.body,
      props: { terminal: target, hasSelection: false },
      global: { plugins: [i18n] },
    });

    (wrapper.vm as unknown as { openSearch(): void }).openSearch();
    await wrapper.vm.$nextTick();
    const input = wrapper.get("input");
    expect(document.activeElement).toBe(input.element);

    await input.setValue("needle");
    expect(target.findNext).toHaveBeenCalledWith("needle", true);
    await input.trigger("keydown", { key: "Enter" });
    expect(target.findNext).toHaveBeenLastCalledWith("needle");
    await input.trigger("keydown", { key: "Enter", shiftKey: true });
    expect(target.findPrevious).toHaveBeenCalledWith("needle");

    await wrapper.get("button[aria-label='Close search']").trigger("click");
    expect(target.clearSearch).toHaveBeenCalled();
    expect(target.focus).toHaveBeenCalled();
    expect(wrapper.find("[role='search']").exists()).toBe(false);
  });

  it("copies only a non-empty explicit terminal selection and reports failures", async () => {
    const wrapper = mount(NvxTerminalTools, {
      attachTo: document.body,
      props: { terminal: target, hasSelection: false },
      global: { plugins: [i18n] },
    });
    const copy = wrapper.get("button[aria-label='Copy selected text']");
    expect(copy.attributes("disabled")).toBeDefined();

    await wrapper.setProps({ hasSelection: true });
    await copy.trigger("click");
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("selected output");
    expect(wrapper.text()).toContain("Copied");

    vi.mocked(navigator.clipboard.writeText).mockRejectedValueOnce(new Error("denied"));
    await copy.trigger("click");
    expect(wrapper.text()).toContain("Copy failed");
  });

  it("keeps search focus out of the surrounding Pane activation handler", async () => {
    const pane = document.createElement("section");
    document.body.append(pane);
    const activatePane = vi.fn();
    pane.addEventListener("pointerdown", activatePane);
    pane.addEventListener("focusin", activatePane);
    const wrapper = mount(NvxTerminalTools, {
      attachTo: pane,
      props: { terminal: target, hasSelection: false },
      global: { plugins: [i18n] },
    });

    await wrapper.get("button[aria-label='Search this terminal']").trigger("pointerdown");
    (wrapper.vm as unknown as { openSearch(): void }).openSearch();
    await wrapper.vm.$nextTick();
    const input = wrapper.get("input");
    await input.trigger("pointerdown");
    expect(document.activeElement).toBe(input.element);
    expect(activatePane).not.toHaveBeenCalled();
    await input.setValue("output only");
    expect(target.findNext).toHaveBeenCalledWith("output only", true);
    wrapper.unmount();
  });
});
