import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import NvxTerminalTabBar, { type TerminalTabItem } from "./NvxTerminalTabBar.vue";

class ResizeObserverStub {
  observe = vi.fn();
  disconnect = vi.fn();
}

const items: TerminalTabItem[] = [
  { groupId: "one", label: "Primary", stateLabel: "Running" },
  { groupId: "two", label: "Secondary", stateLabel: "Disconnected" },
];

describe("NvxTerminalTabBar native drag boundaries", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function mountTabs(tabItems: readonly TerminalTabItem[] = items, dragEnabled = false) {
    return mount(NvxTerminalTabBar, {
      props: {
        items: tabItems,
        dragEnabled,
        modelValue: "one",
        label: "Terminal tabs",
        newLabel: "New terminal",
        closeAllLabel: "Close all",
        closeLeftLabel: "Close left",
        closeRightLabel: "Close right",
        contextMenuLabel: "Tab actions",
      },
      slots: {
        actions: `<div class="test-toolbar" role="toolbar"><button type="button">Layout</button></div>`,
        "trailing-actions": `<button class="test-trailing" type="button">Quick commands</button>`,
      },
    });
  }

  it("covers nested strip and action whitespace with deep drag regions", () => {
    const wrapper = mountTabs();

    for (const selector of [
      ".nvx-terminal-tab-bar",
      ".nvx-terminal-tab-bar__strip",
      ".nvx-terminal-tab-bar__tabs",
      ".nvx-terminal-tab-bar__actions",
    ]) {
      expect(wrapper.get(selector).attributes("data-tauri-drag-region")).toBe("deep");
    }

    expect(wrapper.get(".test-toolbar").attributes("data-tauri-drag-region")).toBeUndefined();
    expect(wrapper.get(".test-toolbar").element.closest("[data-tauri-drag-region]"))
      .toBe(wrapper.findAll(".nvx-terminal-tab-bar__action-island")[0]?.element);
    expect(
      wrapper.get(".test-toolbar").element.closest("[data-tauri-drag-region]")
        ?.getAttribute("data-tauri-drag-region"),
    ).toBe("false");
  });

  it("keeps each tab item and every interactive control outside drag hit testing", () => {
    const wrapper = mountTabs();

    for (const item of wrapper.findAll(".nvx-terminal-tab-bar__item")) {
      expect(item.attributes("data-tauri-drag-region")).toBe("false");
    }

    for (const button of wrapper.findAll("button")) {
      expect(
        button.element.closest("[data-tauri-drag-region]")
          ?.getAttribute("data-tauri-drag-region"),
      ).toBe("false");
    }
  });

  it("preserves click and keyboard tab behavior inside the non-drag island", async () => {
    const wrapper = mountTabs();
    const tabs = wrapper.findAll<HTMLButtonElement>("[role='tab']");

    await tabs[1]?.trigger("click");
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual(["two"]);

    await tabs[0]?.trigger("keydown", { key: "ArrowRight" });
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual(["two"]);

    await wrapper.get(".nvx-terminal-tab-bar__create").trigger("click");
    expect(wrapper.emitted("create")).toHaveLength(1);
  });

  it("renders a labeled, non-color bell-attention marker on a terminal tab", () => {
    const wrapper = mountTabs([{
      ...items[0]!,
      bellAttention: true,
      bellAttentionLabel: "Unseen terminal bell",
    }]);
    const marker = wrapper.get('.nvx-terminal-tab-bar__bell-attention[role="img"]');
    expect(marker.attributes("aria-label")).toBe("Unseen terminal bell");
    expect(marker.find("svg").exists()).toBe(true);
  });

  it("offers bulk close actions from a tab context menu", async () => {
    const wrapper = mountTabs();
    const tabs = wrapper.findAll(".nvx-terminal-tab-bar__item");

    await tabs[1]?.trigger("contextmenu", { clientX: 120, clientY: 40 });
    const menu = wrapper.get('[role="menu"]');
    expect(menu.attributes("aria-label")).toBe("Tab actions");
    expect(menu.get('[role="menuitem"]:first-child').attributes("disabled")).toBeUndefined();
    const right = menu.findAll('[role="menuitem"]')
      .find((item) => item.text() === "Close right")!;
    expect(right.attributes("disabled")).toBeDefined();
    await menu.findAll('[role="menuitem"]')
      .find((item) => item.text() === "Close left")?.trigger("click");
    expect(wrapper.emitted("closeMany")).toEqual([[['one']]]);

    await tabs[0]?.trigger("contextmenu", { clientX: 120, clientY: 40 });
    await wrapper.get('[role="menu"] [role="menuitem"]:last-child').trigger("click");
    expect(wrapper.emitted("closeMany")?.at(-1)).toEqual([["one", "two"]]);
  });

  it("offers an explicit return to main from a detached window", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.setProps({ moveToMainWindowLabel: "Move to main window" });
    await wrapper.findAll(".nvx-terminal-tab-bar__item")[0]?.trigger("contextmenu", { clientX: 120, clientY: 40 });
    await wrapper.findAll('[role="menuitem"]')
      .find((item) => item.text() === "Move to main window")?.trigger("click");
    expect(wrapper.emitted("move-to-main-window")).toEqual([["one"]]);
  });

  it("starts native preview gestures only from enabled tab buttons", async () => {
    const defaultWrapper = mountTabs();
    await defaultWrapper.findAll<HTMLButtonElement>("[role='tab']")[0]?.trigger("pointerdown", { button: 0 });
    expect(defaultWrapper.emitted("tab-pointer-down")).toBeUndefined();
    defaultWrapper.unmount();

    const wrapper = mountTabs(items, true);
    const tabs = wrapper.findAll<HTMLButtonElement>("[role='tab']");
    expect(tabs.every((tab) => tab.attributes("draggable") === undefined)).toBe(true);
    await tabs[0]?.trigger("pointerdown", { button: 2 });
    expect(wrapper.emitted("tab-pointer-down")).toBeUndefined();
    await tabs[0]?.trigger("pointerdown", { button: 0 });
    expect(wrapper.emitted("tab-pointer-down")?.[0]?.[0]).toBe("one");
    await wrapper.findAll(".nvx-terminal-tab-bar__close")[0]?.trigger("pointerdown", { button: 0 });
    expect(wrapper.emitted("tab-pointer-down")).toHaveLength(1);
  });
});
