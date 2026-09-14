import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const terminalMocks = vi.hoisted(() => ({
  instances: [] as Array<{
    options: Record<string, unknown>;
    selectAll: ReturnType<typeof vi.fn>;
    customKeyHandler: ((event: KeyboardEvent) => boolean) | null;
    bellHandler: (() => void) | null;
    linkProvider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void } | null;
  }> ,
  mouseTrackingMode: "none" as "none" | "vt200",
  selection: "",
  line: null as {
    length: number;
    translateToString(trimRight?: boolean): string;
    getCell(column: number): { getWidth(): number; getChars(): string } | undefined;
  } | null,
}));
const opener = vi.hoisted(() => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
const platform = vi.hoisted(() => ({ value: "other" as "macos" | "windows" | "other" }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    modes = { bracketedPasteMode: false, get mouseTrackingMode() { return terminalMocks.mouseTrackingMode; } };
    buffer = { active: { getLine: () => terminalMocks.line } };
    options: Record<string, unknown>;
    customKeyHandler: ((event: KeyboardEvent) => boolean) | null = null;
    bellHandler: (() => void) | null = null;
    linkProvider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void } | null = null;
    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      terminalMocks.instances.push(this);
    }
    loadAddon() {}
    onData() { return { dispose() {} }; }
    onSelectionChange() { return { dispose() {} }; }
    onBell(callback: () => void) {
      this.bellHandler = callback;
      return { dispose: () => { this.bellHandler = null; } };
    }
    attachCustomKeyEventHandler(callback: (event: KeyboardEvent) => boolean) { this.customKeyHandler = callback; }
    registerLinkProvider(provider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void }) {
      this.linkProvider = provider;
      return { dispose: () => { this.linkProvider = null; } };
    }
    open() {}
    focus() {}
    clear() {}
    write() {}
    writeln() {}
    getSelection() { return terminalMocks.selection; }
    hasSelection() { return Boolean(terminalMocks.selection); }
    selectAll = vi.fn();
    dispose() {}
  },
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: opener.openUrl }));
vi.mock("../../platform", () => ({ detectDesktopPlatform: () => platform.value }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {}; findNext() { return false; }; findPrevious() { return false; } } }));
vi.mock("../../terminal/xtermHighlighting", () => ({ createTerminalHighlighter: () => ({ update() {}, dispose() {} }) }));

import NvxTerminalView from "./NvxTerminalView.vue";
import { useUiStore } from "../../stores/ui";
import { useAppThemeStore } from "../../stores/appTheme";
import { BUILTIN_APP_THEMES, cloneDefaultAppThemeProfile } from "../../app-theme";
import { LIGHT_TERMINAL_PALETTE } from "../../terminal-theme";
import * as themeApi from "../../core-api/app-theme";
import { terminalInteractionEn } from "../../locales/terminal-interaction";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { DEFAULT_TERMINAL_INTERACTION, XTERM_DEFAULT_WORD_SEPARATOR } from "../../terminal/interaction-preferences";

class TestResizeObserver {
  observe() {}
  disconnect() {}
}

function terminalBufferLine(text: string) {
  return {
    length: text.length,
    translateToString: () => text,
    getCell: (column: number) => (column >= 0 && column < text.length
      ? { getWidth: () => 1, getChars: () => text[column]! }
      : undefined),
  };
}

describe("NvxTerminalView interaction preferences", () => {
  beforeEach(() => {
    localStorage.clear();
    terminalMocks.instances = [];
    terminalMocks.mouseTrackingMode = "none";
    terminalMocks.selection = "";
    terminalMocks.line = null;
    opener.openUrl.mockClear();
    platform.value = "other";
    vi.stubGlobal("ResizeObserver", TestResizeObserver);
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
  });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("hot-updates public xterm options without recreating the terminal", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const store = useTerminalPreferencesStore(pinia);
    expect(terminalMocks.instances).toHaveLength(1);
    expect(terminalMocks.instances[0]!.options.wordSeparator).toBe(XTERM_DEFAULT_WORD_SEPARATOR);

    store.setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 200, doubleClickSelection: "address" });
    await flushPromises();

    expect(terminalMocks.instances).toHaveLength(1);
    expect(terminalMocks.instances[0]!.options).toMatchObject({ scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 200, wordSeparator: " \t\r\n'`\"<>" });
    store.setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, linksEnabled: false });
    await flushPromises();
    expect(terminalMocks.instances[0]!.options.linkHandler).toBeNull();
    wrapper.unmount();
  });

  it("updates a followed package palette in the same terminal and preserves an explicit palette", async () => {
    const pinia = createPinia();
    const ui = useUiStore(pinia);
    ui.setThemePreference("light");
    ui.setTerminalThemeMode("follow-app");
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const appTheme = useAppThemeStore(pinia);
    const palette = { ...LIGHT_TERMINAL_PALETTE, background: "#faf9f0" };
    const list = vi.spyOn(themeApi, "listPluginThemes").mockResolvedValue({ themes: [{
      pluginId: "org.norishell.test-theme", packageHash: "a".repeat(64), version: "1.0.0", enabled: true,
      definition: { ...BUILTIN_APP_THEMES.light, id: "test", terminalPalette: palette },
    }] });
    await appTheme.refreshThemes();
    const profile = cloneDefaultAppThemeProfile();
    profile.lightThemeId = "plugin:org.norishell.test-theme:test";
    expect(appTheme.saveProfile(profile)).toBe(true);
    await flushPromises();
    expect(terminalMocks.instances).toHaveLength(1);
    expect(terminalMocks.instances[0]!.options.theme).toMatchObject({ background: "#faf9f0" });
    expect(document.documentElement.style.getPropertyValue("--nvx-color-terminal-bg")).toBe("#faf9f0");
    ui.setTerminalThemeMode("custom");
    await flushPromises();
    const explicit = terminalMocks.instances[0]!.options.theme;
    appTheme.saveProfile(cloneDefaultAppThemeProfile());
    await flushPromises();
    expect(terminalMocks.instances).toHaveLength(1);
    expect(terminalMocks.instances[0]!.options.theme).toEqual(explicit);
    expect(wrapper.emitted("input")).toBeUndefined();
    list.mockRestore();
    wrapper.unmount();
  });

  it("forces smooth scrolling off when reduced motion is requested", async () => {
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: true, addEventListener() {}, removeEventListener() {} })));
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, smoothScrollDuration: 200 });
    await flushPromises();
    expect(terminalMocks.instances[0]!.options.smoothScrollDuration).toBe(0);
    wrapper.unmount();
  });

  it("uses the public xterm key hook for BS and consumes an already-reserved app shortcut", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const store = useTerminalPreferencesStore(pinia);
    store.setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, backspaceMode: "bs" });
    await flushPromises();
    const handler = terminalMocks.instances[0]!.customKeyHandler!;

    const backspace = new KeyboardEvent("keydown", { key: "Backspace", code: "Backspace", cancelable: true });
    expect(handler(backspace)).toBe(false);
    expect(backspace.defaultPrevented).toBe(true);
    expect(wrapper.emitted("input")).toContainEqual(["\b"]);

    const reserved = new KeyboardEvent("keydown", { key: "Backspace", code: "Backspace", cancelable: true });
    reserved.preventDefault();
    expect(handler(reserved)).toBe(false);
    expect(wrapper.emitted("input")).toEqual([["\b"]]);
    wrapper.unmount();
  });

  it("applies the selected physical macOS Option side from the per-Host override", async () => {
    platform.value = "macos";
    const pinia = createPinia();
    const store = useTerminalPreferencesStore(pinia);
    store.setHostKeyboard("host-a", {
      mode: "override",
      keyboard: { optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "del" },
    });
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap", hostId: "host-a" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const instance = terminalMocks.instances[0]!;
    const handler = instance.customKeyHandler!;
    expect(instance.options.macOptionIsMeta).toBe(false);
    handler(new KeyboardEvent("keydown", { key: "Alt", code: "AltLeft", altKey: true }));
    expect(instance.options.macOptionIsMeta).toBe(true);
    window.dispatchEvent(new Event("blur"));
    handler(new KeyboardEvent("keydown", { key: "e", code: "KeyE", altKey: true }));
    expect(instance.options.macOptionIsMeta).toBe(false);
    handler(new KeyboardEvent("keydown", { key: "Alt", code: "AltLeft", altKey: true }));
    handler(new KeyboardEvent("keyup", { key: "Alt", code: "AltLeft", altKey: false }));
    handler(new KeyboardEvent("keydown", { key: "Alt", code: "AltRight", altKey: true }));
    expect(instance.options.macOptionIsMeta).toBe(false);
    wrapper.unmount();
  });

  it("marks an unfocused pane on BEL and clears the marker on focus", async () => {
    const focusSpy = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, bellMode: "visual" });
    await flushPromises();
    terminalMocks.instances[0]!.bellHandler?.();
    await wrapper.vm.$nextTick();

    const root = wrapper.find(".nvx-terminal-view");
    expect(root.classes()).toContain("nvx-terminal-view--bell-flash");
    expect(wrapper.emitted("bellAttention")).toContainEqual([true]);
    await root.trigger("focusin");
    expect(wrapper.emitted("bellAttention")).toContainEqual([false]);
    focusSpy.mockRestore();
    wrapper.unmount();
  });

  it("copies only the selection captured when a document mouse gesture ends", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, copyOnSelect: true });
    const root = wrapper.find(".nvx-terminal-view");
    terminalMocks.selection = "original selection";
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "selected output";
    document.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
    terminalMocks.selection = "later selection";
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(1);
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("selected output");
    wrapper.unmount();
  });

  it("completes an active selection when mouseup is dispatched on window", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, copyOnSelect: true });
    const root = wrapper.find(".nvx-terminal-view");
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "released outside view";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("released outside view");
    wrapper.unmount();
  });

  it("waits for the left mouse button before ending an active selection", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, copyOnSelect: true });
    await wrapper.find(".nvx-terminal-view").trigger("mousedown", { button: 0 });
    terminalMocks.selection = "still selecting";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 2 }));
    await flushPromises();
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();

    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("still selecting");
    wrapper.unmount();
  });

  it("does not copy without an eligible mouse selection gesture", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, copyOnSelect: true });
    const root = wrapper.find(".nvx-terminal-view");

    terminalMocks.selection = "without gesture";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await root.trigger("mousedown", { button: 0, shiftKey: true });
    terminalMocks.selection = "shift selection";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "shift released selection";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0, shiftKey: true }));
    terminalMocks.mouseTrackingMode = "vt200";
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "tui selection";
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await flushPromises();

    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("cancels a pending mouse gesture on pointer cancellation, blur, and unmount", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    useTerminalPreferencesStore(pinia).setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, copyOnSelect: true });
    const root = wrapper.find(".nvx-terminal-view");

    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "cancelled";
    await root.trigger("pointercancel");
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "blurred";
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await root.trigger("mousedown", { button: 0 });
    terminalMocks.selection = "unmounted";
    wrapper.unmount();
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
    await flushPromises();

    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  });

  it("uses the shared popover lifecycle for terminal context actions and leaves TUI mouse reports untouched", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    terminalMocks.selection = "selected output";
    const root = wrapper.find(".nvx-terminal-view");
    await root.trigger("contextmenu", { clientX: 24, clientY: 24 });
    expect(wrapper.find('[role="menu"]').exists()).toBe(true);
    await wrapper.findAll('[role="menuitem"]')[2]!.trigger("click");
    expect(terminalMocks.instances[0]!.selectAll).toHaveBeenCalledOnce();
    await root.trigger("keydown", { key: "ContextMenu" });
    expect(wrapper.find('[role="menu"]').exists()).toBe(true);
    await wrapper.find('[role="menu"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);

    terminalMocks.mouseTrackingMode = "vt200";
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    wrapper.element.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("opens only modifier-clicked safe links, copies their hover snapshot, and leaves TUI input alone", async () => {
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const url = "https://example.test/path?q=1";
    terminalMocks.line = terminalBufferLine(url);
    const provider = terminalMocks.instances[0]!.linkProvider!;
    let provided: Array<{
      activate(event: MouseEvent): void;
      hover(event: MouseEvent): void;
    }> | undefined;
    provider.provideLinks(1, (links) => { provided = links as typeof provided; });
    expect(provided).toHaveLength(1);
    const link = provided![0]!;

    const ordinaryClick = new MouseEvent("click", { cancelable: true });
    link.activate(ordinaryClick);
    await flushPromises();
    expect(opener.openUrl).not.toHaveBeenCalled();
    expect(ordinaryClick.defaultPrevented).toBe(false);

    const modifiedClick = new MouseEvent("click", { ctrlKey: true, cancelable: true });
    link.activate(modifiedClick);
    await flushPromises();
    expect(modifiedClick.defaultPrevented).toBe(true);
    expect(opener.openUrl).toHaveBeenCalledWith(url);
    expect(wrapper.emitted("input")).toBeUndefined();

    link.hover(new MouseEvent("mousemove"));
    await wrapper.find(".nvx-terminal-view").trigger("contextmenu", { clientX: 24, clientY: 24 });
    const copyLink = wrapper.findAll('[role="menuitem"]').find((item) => item.text() === "Copy link")!;
    await copyLink.trigger("click");
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(url);
    expect(wrapper.emitted("input")).toBeUndefined();

    terminalMocks.mouseTrackingMode = "vt200";
    provider.provideLinks(1, (links) => { provided = links as typeof provided; });
    expect(provided).toBeUndefined();
    wrapper.unmount();
  });
});
