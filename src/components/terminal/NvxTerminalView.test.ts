import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const terminalMocks = vi.hoisted(() => ({
  instances: [] as Array<{
    cols: number;
    buffer: { active: { type: string } };
    options: Record<string, unknown>;
    textarea: HTMLTextAreaElement;
    dataHandler: ((value: string) => void) | null;
    writeParsedHandler: (() => void) | null;
    selectAll: ReturnType<typeof vi.fn>;
    customKeyHandler: ((event: KeyboardEvent) => boolean) | null;
    bellHandler: (() => void) | null;
    linkProvider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void } | null;
  }> ,
  mouseTrackingMode: "none" as "none" | "vt200",
  selection: "",
  cursorX: 0,
  cursorY: 0,
  lines: null as Record<number, ReturnType<typeof terminalBufferLine>> | null,
  line: null as {
    length: number;
    isWrapped?: boolean;
    translateToString(trimRight?: boolean, start?: number, end?: number): string;
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
    buffer = {
      normal: { type: "normal", baseY: 0, get cursorY() { return terminalMocks.cursorY; }, get cursorX() { return terminalMocks.cursorX; }, getLine: (index: number) => terminalMocks.lines?.[index] ?? terminalMocks.line },
      active: { type: "normal", baseY: 0, get cursorY() { return terminalMocks.cursorY; }, get cursorX() { return terminalMocks.cursorX; }, getLine: (index: number) => terminalMocks.lines?.[index] ?? terminalMocks.line },
    };
    options: Record<string, unknown>;
    customKeyHandler: ((event: KeyboardEvent) => boolean) | null = null;
    bellHandler: (() => void) | null = null;
    linkProvider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void } | null = null;
    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      terminalMocks.instances.push(this);
    }
    loadAddon() {}
    textarea = document.createElement("textarea");
    dataHandler: ((value: string) => void) | null = null;
    writeParsedHandler: (() => void) | null = null;
    onData(callback: (value: string) => void) { this.dataHandler = callback; return { dispose() {} }; }
    onSelectionChange() { return { dispose() {} }; }
    onWriteParsed(callback: () => void) { this.writeParsedHandler = callback; return { dispose: () => { this.writeParsedHandler = null; } }; }
    onResize() { return { dispose() {} }; }
    onBell(callback: () => void) {
      this.bellHandler = callback;
      return { dispose: () => { this.bellHandler = null; } };
    }
    attachCustomKeyEventHandler(callback: (event: KeyboardEvent) => boolean) { this.customKeyHandler = callback; }
    registerLinkProvider(provider: { provideLinks: (line: number, callback: (links: unknown[] | undefined) => void) => void }) {
      this.linkProvider = provider;
      return { dispose: () => { this.linkProvider = null; } };
    }
    open(host: HTMLElement) { host.append(this.textarea); }
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
    translateToString: (trimRight = false, start = 0, end = text.length) => trimRight ? text.slice(start, end).trimEnd() : text.slice(start, end),
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
    terminalMocks.cursorX = 0;
    terminalMocks.cursorY = 0;
    terminalMocks.line = null;
    terminalMocks.lines = null;
    opener.openUrl.mockClear();
    platform.value = "other";
    vi.stubGlobal("ResizeObserver", TestResizeObserver);
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
  });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("rejects residual input or an unknown Shell prompt", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const view = wrapper.vm as unknown as { isEmptyShellPrompt(): boolean };
    const inspect = (line: string, cursorX = line.length) => {
      terminalMocks.line = terminalBufferLine(line);
      terminalMocks.cursorX = cursorX;
      return view.isEmptyShellPrompt();
    };
    expect(inspect("[root@server yy]# ")).toBe(true);
    expect(inspect("[root@server yy]# cd ")).toBe(false);
    expect(inspect("[root@server yy]# cd ", "[root@server yy]# ".length)).toBe(false);
    expect(inspect("user@host:~$ ")).toBe(true);
    expect(inspect("PS C:\\Users\\Administrator> ")).toBe(true);
    expect(inspect("unknown prompt ")).toBe(false);
    terminalMocks.line = { ...terminalBufferLine("# "), isWrapped: true };
    terminalMocks.cursorX = 2;
    expect(view.isEmptyShellPrompt()).toBe(false);
    wrapper.unmount();
  });

  it("records a visible simple Shell line and suppresses Codex input in the same Pane", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const type = (command: string) => {
      const prompt = "[root@server yy]# ";
      let text = prompt;
      for (const character of command) {
        terminalMocks.line = terminalBufferLine(text);
        terminalMocks.cursorX = text.length;
        terminalMocks.instances[0]!.dataHandler?.(character);
        text += character;
      }
      terminalMocks.line = terminalBufferLine(text);
      terminalMocks.cursorX = text.length;
      terminalMocks.instances[0]!.dataHandler?.("\r");
    };
    type("cd /www/wwwroot");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/wwwroot"]);
    type("codex");
    type("secret inside app");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r"]);
    expect(wrapper.emitted("draftChange")?.at(-1)).toEqual([null]);
    wrapper.unmount();
  });

  it("resumes history only after an alternate screen exits to a new strong Shell prompt", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const prompt = "[root@server yy]# ";
    const type = (command: string) => {
      let line = prompt;
      for (const character of command) {
        terminalMocks.line = terminalBufferLine(line);
        terminalMocks.cursorX = line.length;
        term.dataHandler?.(character);
        line += character;
      }
      terminalMocks.line = terminalBufferLine(line);
      terminalMocks.cursorX = line.length;
      term.dataHandler?.("\r");
      return wrapper.emitted("input")?.at(-1);
    };
    expect(type("codex")).toEqual(["\r", "codex"]);
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    terminalMocks.cursorY = 1;
    term.writeParsedHandler?.();
    expect(type("secret inside app")).toEqual(["\r"]);

    term.buffer.active.type = "alternate";
    terminalMocks.cursorY = 0;
    term.writeParsedHandler?.();
    expect(type("another secret")).toEqual(["\r"]);
    term.buffer.active.type = "normal";
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    term.writeParsedHandler?.();
    expect(type("still in app")).toEqual(["\r"]);

    terminalMocks.cursorY = 1;
    terminalMocks.line = terminalBufferLine("unrecognized > ");
    terminalMocks.cursorX = "unrecognized > ".length;
    term.writeParsedHandler?.();
    expect(type("not shell input")).toEqual(["\r"]);
    terminalMocks.cursorY = 2;
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    term.writeParsedHandler?.();
    expect(type("cd /www/wwwroot")).toEqual(["\r", "cd /www/wwwroot"]);
    wrapper.unmount();
  });

  it("keeps another full-screen program out of history until it returns to Shell", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const prompt = "[root@server yy]# ";
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    term.dataHandler?.("v");
    terminalMocks.line = terminalBufferLine(`${prompt}vim`);
    terminalMocks.cursorX = `${prompt}vim`.length;
    term.dataHandler?.("\r");
    term.buffer.active.type = "alternate";
    term.writeParsedHandler?.();
    term.dataHandler?.("a");
    term.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r"]);
    term.buffer.active.type = "normal";
    terminalMocks.cursorY = 1;
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    term.writeParsedHandler?.();
    let line = prompt;
    for (const character of "cd /www/wwwroot") {
      terminalMocks.line = terminalBufferLine(line);
      terminalMocks.cursorX = line.length;
      term.dataHandler?.(character);
      line += character;
    }
    terminalMocks.line = terminalBufferLine(line);
    terminalMocks.cursorX = line.length;
    term.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/wwwroot"]);
    wrapper.unmount();
  });

  it("keeps a simple command when a narrow pane wraps its prompt and path", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    term.cols = 37;
    const show = (text: string) => {
      const chunks = text.match(/.{1,37}/g) ?? [""];
      if (text.length % term.cols === 0) chunks.push("");
      terminalMocks.lines = Object.fromEntries(chunks.map((chunk, index) => [index, { ...terminalBufferLine(chunk), isWrapped: index > 0 }]));
      terminalMocks.cursorY = chunks.length - 1;
      terminalMocks.cursorX = chunks.at(-1)!.length;
    };
    const type = (prompt: string, command: string) => {
      let text = prompt;
      for (const character of command) {
        show(text);
        term.dataHandler?.(character);
        text += character;
      }
      show(text);
      term.dataHandler?.("\r");
    };
    type("root@ip-172-26-7-37:~# ", "cd /www/wwwroot/NorixorAI/");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/wwwroot/NorixorAI/"]);
    type("root@ip-172-26-7-37:/www/wwwroot/NorixorAI# ", "cd /www/w");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/w"]);
    wrapper.unmount();
  });

  it("captures a Shell Tab completion only when the echoed command retains the typed text", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const prompt = "[root@server yy]# ";
    let echoed = prompt;
    for (const character of "cd /www/") {
      terminalMocks.line = terminalBufferLine(echoed);
      terminalMocks.cursorX = echoed.length;
      terminalMocks.instances[0]!.dataHandler?.(character);
      echoed += character;
    }
    terminalMocks.line = terminalBufferLine(echoed);
    terminalMocks.cursorX = echoed.length;
    terminalMocks.instances[0]!.dataHandler?.("\t");
    expect(wrapper.emitted("draftChange")?.at(-1)).toEqual([null]);
    echoed = `${prompt}cd /www/wwwroot/`;
    for (const character of "NorixorAI/") {
      terminalMocks.line = terminalBufferLine(echoed);
      terminalMocks.cursorX = echoed.length;
      terminalMocks.instances[0]!.dataHandler?.(character);
      echoed += character;
    }
    terminalMocks.line = terminalBufferLine(echoed);
    terminalMocks.cursorX = echoed.length;
    terminalMocks.instances[0]!.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/wwwroot/NorixorAI/"]);

    echoed = prompt;
    for (const character of "cd /etc/") {
      terminalMocks.line = terminalBufferLine(echoed);
      terminalMocks.cursorX = echoed.length;
      terminalMocks.instances[0]!.dataHandler?.(character);
      echoed += character;
    }
    terminalMocks.instances[0]!.dataHandler?.("\t");
    terminalMocks.line = terminalBufferLine(`${prompt}pwd`);
    terminalMocks.cursorX = `${prompt}pwd`.length;
    terminalMocks.instances[0]!.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r"]);
    wrapper.unmount();
  });

  it("recovers a Shell Tab command from its visible prompt when earlier keystrokes were not tracked", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const prompt = "root@ip-172-26-7-37:~# ";
    terminalMocks.line = terminalBufferLine(`${prompt}cd /www/S`);
    terminalMocks.cursorX = `${prompt}cd /www/S`.length;
    terminalMocks.instances[0]!.dataHandler?.("\t");
    terminalMocks.line = terminalBufferLine(`${prompt}cd /www/SyncServer/`);
    terminalMocks.cursorX = `${prompt}cd /www/SyncServer/`.length;
    terminalMocks.instances[0]!.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www/SyncServer/"]);
    wrapper.unmount();
  });

  it("accepts delayed SSH echo after a Shell clear while keeping a mismatched echo out of history", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const view = wrapper.vm as unknown as { writeBytes(bytes: number[]): void };
    view.writeBytes([27, 91, 72, 27, 91, 50, 74]);
    const prompt = "root@ip-172-26-7-37:/# ";
    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    for (const character of "cd /www") terminalMocks.instances[0]!.dataHandler?.(character);
    terminalMocks.line = terminalBufferLine(`${prompt}cd /`);
    terminalMocks.cursorX = `${prompt}cd /`.length;
    terminalMocks.instances[0]!.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r", "cd /www"]);

    terminalMocks.line = terminalBufferLine(prompt);
    terminalMocks.cursorX = prompt.length;
    for (const character of "cd /etc") terminalMocks.instances[0]!.dataHandler?.(character);
    terminalMocks.line = terminalBufferLine(`${prompt}pwd`);
    terminalMocks.cursorX = `${prompt}pwd`.length;
    terminalMocks.instances[0]!.dataHandler?.("\r");
    expect(wrapper.emitted("input")?.at(-1)).toEqual(["\r"]);
    wrapper.unmount();
  });

  it("consumes disconnected typing and paste without forwarding terminal-generated data", async () => {
    const wrapper = mount(NvxTerminalView, {
      attachTo: document.body,
      props: { readOnly: true, reconnectOnInput: true, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    term.textarea.focus();
    const hasFocus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
    expect(term.options.disableStdin).toBe(true);
    term.dataHandler?.("\x1b[?1;2c");
    expect(wrapper.emitted("reconnectRequest")).toBeUndefined();
    for (const key of ["x", "Enter", "Process", "Dead"]) {
      const event = new KeyboardEvent("keydown", { key, cancelable: true });
      expect(term.customKeyHandler?.(event)).toBe(false);
      expect(event.defaultPrevented).toBe(true);
    }
    expect(wrapper.emitted("reconnectRequest")).toHaveLength(4);
    for (const init of [{ key: "ArrowUp" }, { key: "Shift" }, { key: "c", metaKey: true }, { key: "c", ctrlKey: true }, { key: "x", repeat: true }]) {
      term.customKeyHandler?.(new KeyboardEvent("keydown", init));
    }
    term.customKeyHandler?.(new KeyboardEvent("keyup", { key: "x" }));
    expect(wrapper.emitted("reconnectRequest")).toHaveLength(4);
    const clipboard = new DataTransfer();
    clipboard.setData("text/plain", "do not execute\n");
    term.textarea.dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
    expect(wrapper.emitted("reconnectRequest")).toHaveLength(5);
    const modal = document.createElement("div");
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    document.body.append(modal);
    term.customKeyHandler?.(new KeyboardEvent("keydown", { key: "x" }));
    modal.remove();
    term.textarea.blur();
    term.customKeyHandler?.(new KeyboardEvent("keydown", { key: "x" }));
    term.textarea.focus();
    await wrapper.setProps({ reconnectOnInput: false });
    term.customKeyHandler?.(new KeyboardEvent("keydown", { key: "x" }));
    expect(wrapper.emitted("reconnectRequest")).toHaveLength(5);
    await wrapper.setProps({ reconnectOnInput: true });
    (wrapper.vm as unknown as { pasteFromClipboard(): void }).pasteFromClipboard();
    expect(wrapper.emitted("reconnectRequest")).toHaveLength(6);
    expect(wrapper.emitted("input")).toBeUndefined();
    await wrapper.setProps({ readOnly: false });
    term.dataHandler?.("new input");
    expect(wrapper.emitted("input")).toEqual([["new input"]]);
    hasFocus.mockRestore();
    wrapper.unmount();
  });

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
    await root.find('[role="menuitem"]').trigger("pointerdown");
    expect(wrapper.find('[role="menu"]').exists()).toBe(true);
    await root.find('.nvx-terminal-view__host').trigger("pointerdown");
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    await root.trigger("contextmenu", { clientX: 24, clientY: 24 });
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
