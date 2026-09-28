import { mount } from "@vue/test-utils";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const nativeMenu = vi.hoisted(() => ({
  create: vi.fn(),
  popup: vi.fn(async () => undefined),
  close: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "main" }) }));
vi.mock("@tauri-apps/api/menu", () => ({ Menu: { new: nativeMenu.create } }));


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
    nativeMenu.create.mockReset().mockImplementation(async () => ({
      popup: nativeMenu.popup, close: nativeMenu.close,
    }));
    nativeMenu.popup.mockReset().mockResolvedValue(undefined);
    nativeMenu.close.mockReset().mockResolvedValue(undefined);
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

  it("keeps compact Page tabs readable and preserves their full title", () => {
    const wrapper = mountTabs([{ ...items[0]!, compact: true }]);
    expect(wrapper.find(".nvx-terminal-tab-bar__item--compact").exists()).toBe(true);
    expect(wrapper.get("[role='tab']").attributes("title")).toBe("Primary · Running");
  });

  it("shows an inert placeholder only while another Tab hovers", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.setProps({ incomingDrag: true });
    const placeholder = wrapper.get(".nvx-terminal-tab-bar__incoming");
    expect(placeholder.attributes("aria-hidden")).toBe("true");
    expect(placeholder.attributes("data-tauri-drag-region")).toBe("false");
    await wrapper.setProps({ incomingDrag: false });
    expect(wrapper.find(".nvx-terminal-tab-bar__incoming").exists()).toBe(false);
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

  type NativeMenuOptions = { items: { id?: string; text?: string; enabled?: boolean; action?: () => void }[] };
  async function openNativeMenu(wrapper: ReturnType<typeof mountTabs>, index: number) {
    const before = nativeMenu.create.mock.calls.length;
    await wrapper.findAll(".nvx-terminal-tab-bar__item")[index]?.trigger("contextmenu", { clientX: 120, clientY: 40 });
    await vi.waitFor(() => expect(nativeMenu.close).toHaveBeenCalledTimes(before + 1));
    return nativeMenu.create.mock.calls[before]![0] as NativeMenuOptions;
  }

  it("offers bulk close actions from the native tab menu", async () => {
    const wrapper = mountTabs();
    const second = await openNativeMenu(wrapper, 1);
    expect(second.items.find((item) => item.text === "Close right")?.enabled).toBe(false);
    second.items.find((item) => item.text === "Close left")?.action?.();
    expect(wrapper.emitted("closeMany")).toEqual([[["one"]]]);

    const first = await openNativeMenu(wrapper, 0);
    first.items.find((item) => item.text === "Close all")?.action?.();
    expect(wrapper.emitted("closeMany")?.at(-1)).toEqual([["one", "two"]]);
  });

  it("offers an explicit return to main from a detached window", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.setProps({ moveToMainWindowLabel: "Move to main window" });
    const options = await openNativeMenu(wrapper, 0);
    options.items.find((item) => item.text === "Move to main window")?.action?.();
    expect(wrapper.emitted("move-to-main-window")).toEqual([["one"]]);
  });

  it("shows the shell menu above child WebViews through the native window", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.findAll(".nvx-terminal-tab-bar__item")[1]?.trigger("contextmenu", { clientX: 120, clientY: 40 });
    await vi.waitFor(() => expect(nativeMenu.popup).toHaveBeenCalledWith(new LogicalPosition(120, 40), { label: "main" }));
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    const options = nativeMenu.create.mock.calls[0]![0] as {
      items: { text?: string; enabled?: boolean; action?: () => void }[];
    };
    expect(options.items.find((item) => item.text === "Close right")?.enabled).toBe(false);
    options.items.find((item) => item.text === "Close left")?.action?.();
    expect(wrapper.emitted("closeMany")?.at(-1)).toEqual([["one"]]);
    wrapper.unmount();
  });

  it("reuses fixed native menu ids so repeated popups replace action channels", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.setProps({ moveToMainWindowLabel: "Move to main window" });
    const tabs = wrapper.findAll(".nvx-terminal-tab-bar__item");
    await tabs[0]?.trigger("contextmenu", { clientX: 10, clientY: 10 });
    await vi.waitFor(() => expect(nativeMenu.close).toHaveBeenCalledOnce());
    await tabs[1]?.trigger("contextmenu", { clientX: 10, clientY: 10 });
    await vi.waitFor(() => expect(nativeMenu.close).toHaveBeenCalledTimes(2));
    type Options = { id?: string; items: { id?: string; item?: string; action?: () => void }[] };
    const [first, second] = nativeMenu.create.mock.calls.map((call) => call[0] as Options);
    const ids = (options: Options) => [options.id, ...options.items.filter((item) => !item.item).map((item) => item.id)];
    expect(ids(first!)).toEqual([
      "norishell.terminal-tab-menu",
      "norishell.terminal-tab-menu.move-to-new-window",
      "norishell.terminal-tab-menu.move-to-main-window",
      "norishell.terminal-tab-menu.close-left",
      "norishell.terminal-tab-menu.close-right",
      "norishell.terminal-tab-menu.close-all",
    ]);
    expect(ids(second!)).toEqual(ids(first!));
    // Actions still target the Tab the menu was opened for, even after close.
    second!.items.find((item) => item.id?.endsWith(".move-to-main-window"))?.action?.();
    expect(wrapper.emitted("move-to-main-window")).toEqual([["two"]]);
    wrapper.unmount();
  });

  it("opens the same native tab menu from the keyboard", async () => {
    const wrapper = mountTabs(items, true);
    await wrapper.findAll("[role='tab']")[0]?.trigger("keydown", { key: "F10", shiftKey: true });
    await vi.waitFor(() => expect(nativeMenu.popup).toHaveBeenCalledWith(expect.any(LogicalPosition), { label: "main" }));
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("keeps a native popup resource open until the popup finishes", async () => {
    let dismiss!: () => void;
    nativeMenu.popup.mockReturnValue(new Promise<undefined>((resolve) => { dismiss = () => resolve(undefined); }));
    const wrapper = mountTabs(items, true);
    const tab = wrapper.findAll(".nvx-terminal-tab-bar__item")[0]!;
    await tab.trigger("contextmenu");
    await vi.waitFor(() => expect(nativeMenu.popup).toHaveBeenCalledOnce());
    await tab.trigger("contextmenu");
    wrapper.unmount();
    expect(nativeMenu.create).toHaveBeenCalledOnce();
    expect(nativeMenu.close).not.toHaveBeenCalled();
    dismiss();
    await vi.waitFor(() => expect(nativeMenu.close).toHaveBeenCalledOnce());
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
