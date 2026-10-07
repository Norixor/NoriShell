import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const terminalMocks = vi.hoisted(() => ({
  instances: [] as Array<{
    cols: number;
    buffer: { active: { type: string; viewportY: number } };
    element: HTMLElement;
    scrollLines: ReturnType<typeof vi.fn>;
    options: Record<string, unknown>;
    textarea: HTMLTextAreaElement;
    dataHandler: ((value: string) => void) | null;
    writeParsedHandler: (() => void) | null;
    selectAll: ReturnType<typeof vi.fn>;
    select: ReturnType<typeof vi.fn>;
    refresh: ReturnType<typeof vi.fn>;
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
const platform = vi.hoisted(() => ({ value: "other" as "macos" | "windows" | "linux" | "android" | "other" }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    modes = { bracketedPasteMode: false, get mouseTrackingMode() { return terminalMocks.mouseTrackingMode; } };
    buffer = {
      normal: { type: "normal", baseY: 0, get cursorY() { return terminalMocks.cursorY; }, get cursorX() { return terminalMocks.cursorX; }, getLine: (index: number) => terminalMocks.lines?.[index] ?? terminalMocks.line },
      active: { type: "normal", baseY: 0, viewportY: 0, get cursorY() { return terminalMocks.cursorY; }, get cursorX() { return terminalMocks.cursorX; }, getLine: (index: number) => terminalMocks.lines?.[index] ?? terminalMocks.line },
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
    element = document.createElement("div");
    scrollLines = vi.fn((lines: number) => { this.buffer.active.viewportY = Math.max(0, this.buffer.active.viewportY + lines); });
    open(host: HTMLElement) {
      this.element.className = "xterm";
      const screen = document.createElement("div");
      screen.className = "xterm-screen";
      this.element.append(screen);
      host.append(this.textarea, this.element);
    }
    focus() {}
    clear() {}
    write() {}
    writeln() {}
    getSelection() { return terminalMocks.selection; }
    hasSelection() { return Boolean(terminalMocks.selection); }
    selectAll = vi.fn();
    select = vi.fn();
    clearSelection = vi.fn();
    input(data: string) { if (!this.options.disableStdin) this.dataHandler?.(data); }
    refresh = vi.fn();
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
import { useTipsStore } from "../../stores/tips";
import { FitAddon } from "@xterm/addon-fit";
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

  it("repaints on the first visible fit and when a native Tab becomes visible again", async () => {
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    Object.defineProperty(wrapper.find(".nvx-terminal-view__host").element, "clientWidth", { value: 800 });
    const renderer = terminalMocks.instances[0]!;
    window.dispatchEvent(new CustomEvent("norishell:workspace-tab-visibility", { detail: false }));
    await new Promise((resolve) => setTimeout(resolve, 25));
    expect(renderer.refresh).not.toHaveBeenCalled();
    wrapper.vm.fit();
    expect(renderer.refresh).toHaveBeenCalledWith(0, 23);
    renderer.refresh.mockClear();
    window.dispatchEvent(new CustomEvent("norishell:workspace-tab-visibility", { detail: true }));
    await new Promise((resolve) => setTimeout(resolve, 75));
    expect(renderer.refresh.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(renderer.refresh).toHaveBeenLastCalledWith(0, 23);
    wrapper.unmount();
  });

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
    expect(await appTheme.saveProfile(profile)).toBe(true);
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
    expect(terminalMocks.instances[0]!.options.cursorBlink).toBe(false);
    wrapper.unmount();
  });

  it("keeps cursor blink in sync with the system motion preference", async () => {
    const media = { matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() };
    vi.stubGlobal("matchMedia", vi.fn(() => media));
    const pinia = createPinia();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const ui = useUiStore(pinia);
    const terminal = terminalMocks.instances[0]!;
    expect(terminal.options.cursorBlink).toBe(true);

    media.matches = true;
    const onSystemChange = media.addEventListener.mock.calls.find(([event]) => event === "change")?.[1] as (() => void) | undefined;
    expect(onSystemChange).toBeDefined();
    onSystemChange?.();
    expect(terminal.options.cursorBlink).toBe(false);

    media.matches = false;
    onSystemChange?.();
    expect(terminal.options.cursorBlink).toBe(true);
    ui.terminalCursorBlink = false;
    await flushPromises();
    expect(terminal.options.cursorBlink).toBe(false);
    expect(terminalMocks.instances).toHaveLength(1);
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

describe("NvxTerminalView on the phone layout", () => {
  afterEach(() => { platform.value = "other"; });

  it("marks the xterm textarea as a terminal field and applies sticky modifiers to typed data", async () => {
    platform.value = "android";
    const { clearStickyModifiers, toggleStickyCtrl, toggleStickyAlt } = await import("../../terminal/extra-keys");
    clearStickyModifiers();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const renderer = terminalMocks.instances.at(-1)!;
    expect(renderer.textarea.getAttribute("autocapitalize")).toBe("off");
    expect(renderer.textarea.getAttribute("autocorrect")).toBe("off");
    expect(renderer.textarea.getAttribute("spellcheck")).toBe("false");
    expect(renderer.textarea.getAttribute("enterkeyhint")).toBe("enter");
    renderer.dataHandler?.("c");
    toggleStickyCtrl();
    renderer.dataHandler?.("c");
    toggleStickyAlt();
    renderer.dataHandler?.("x");
    renderer.dataHandler?.("x");
    const inputs = (wrapper.emitted("input") ?? []).map((args) => args[0]);
    expect(inputs).toEqual(["c", "\x03", "\x1bx", "x"]);
    expect((wrapper.vm as unknown as { applicationCursorKeys(): boolean }).applicationCursorKeys()).toBe(false);
  });

  it("leaves desktop typed data and the textarea untouched", async () => {
    const { toggleStickyCtrl, clearStickyModifiers } = await import("../../terminal/extra-keys");
    clearStickyModifiers();
    const wrapper = mount(NvxTerminalView, {
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
    await flushPromises();
    const renderer = terminalMocks.instances.at(-1)!;
    expect(renderer.textarea.getAttribute("enterkeyhint")).toBeNull();
    toggleStickyCtrl();
    renderer.dataHandler?.("c");
    expect((wrapper.emitted("input") ?? []).map((args) => args[0])).toEqual(["c"]);
    clearStickyModifiers();
  });
});

describe("NvxTerminalView on the phone shell", () => {
  beforeEach(() => {
    localStorage.clear();
    terminalMocks.instances = [];
    terminalMocks.mouseTrackingMode = "none";
    terminalMocks.selection = "";
    platform.value = "android";
    vi.stubGlobal("ResizeObserver", TestResizeObserver);
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })));
  });
  afterEach(() => {
    platform.value = "other";
    vi.unstubAllGlobals();
  });

  function mountMobile(readOnly = false) {
    return mount(NvxTerminalView, {
      attachTo: document.body,
      props: { readOnly, terminalLabel: "Terminal", gapLabel: "Gap" },
      global: { plugins: [createPinia(), createI18n({ legacy: false, locale: "en", missingWarn: false, fallbackWarn: false, messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
  }

  it("owns soft-keyboard text so keyCode 229 never reaches xterm's diffing path", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    expect(term.customKeyHandler?.(new KeyboardEvent("keydown", { key: "Unidentified", keyCode: 229 }))).toBe(false);
    expect(term.customKeyHandler?.(new KeyboardEvent("keydown", { key: "Enter", keyCode: 13 }))).toBe(true);
    for (const character of "ls") {
      term.textarea.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, data: character, inputType: "insertText" }));
      term.textarea.value += character;
      term.textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: character, inputType: "insertText" }));
    }
    expect(wrapper.emitted("input")?.map(([value]) => value)).toEqual(["l", "s"]);
    wrapper.unmount();
  });

  it("drops soft-keyboard text while read-only", async () => {
    const wrapper = mountMobile(true);
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    term.textarea.value = "x";
    term.textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: "x", inputType: "insertText" }));
    expect(wrapper.emitted("input")).toBeUndefined();
    wrapper.unmount();
  });

  it("never types a multi-line IME commit; it goes to the guarded paste path", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    term.textarea.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, data: "rm -rf x\nreboot\n", inputType: "insertText" }));
    term.textarea.value = "rm -rf x\nreboot\n";
    term.textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: "rm -rf x\nreboot\n", inputType: "insertText" }));
    await flushPromises();
    expect(wrapper.emitted("input")).toBeUndefined();
    wrapper.unmount();
  });

  it("returns keyboard-scrolled ancestors to the origin while the terminal owns focus", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const root = wrapper.find(".nvx-terminal-view").element as HTMLElement;
    const parent = root.parentElement!;
    term.textarea.focus();
    root.scrollTop = 262;
    parent.scrollTop = 41;
    root.dispatchEvent(new Event("scroll"));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(root.scrollTop).toBe(0);
    expect(parent.scrollTop).toBe(0);
    term.textarea.blur();
    parent.scrollTop = 30;
    parent.dispatchEvent(new Event("scroll"));
    await new Promise((resolve) => setTimeout(resolve, 50));
    // Without keyboard focus the terminal leaves shared containers alone.
    expect(parent.scrollTop).toBe(30);
    wrapper.unmount();
  });

  it("suppresses the WebView long-press menu and offers in-view selection actions", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    wrapper.find(".nvx-terminal-view").element.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(wrapper.find(".nvx-terminal-view__context-menu").exists()).toBe(false);
    await wrapper.find(".nvx-terminal-view").trigger("keydown", { key: "ContextMenu" });
    const actions = wrapper.findAll(".nvx-terminal-view__touch-action");
    expect(actions.map((action) => action.text()).slice(0, 3)).toEqual(["Copy", "Paste", "Select all"]);
    await actions[2]!.trigger("click");
    expect(terminalMocks.instances[0]!.selectAll).toHaveBeenCalled();
    wrapper.unmount();
  });

  /** A plain finger tap on the terminal content, as the capture-phase touch listeners see it. */
  function tap(target: Element) {
    const touch = { identifier: 7, clientX: 40, clientY: 40 };
    const start = new Event("touchstart", { bubbles: true });
    Object.defineProperty(start, "touches", { value: [touch] });
    Object.defineProperty(start, "changedTouches", { value: [touch] });
    target.dispatchEvent(start);
    const end = new Event("touchend", { bubbles: true, cancelable: true });
    Object.defineProperty(end, "touches", { value: [] });
    Object.defineProperty(end, "changedTouches", { value: [touch] });
    target.dispatchEvent(end);
  }

  /** One finger dragged through `points` ([x, y, timeStamp]); returns the touchend event. */
  function drag(target: Element, points: Array<[number, number, number]>) {
    const touchEvent = (type: string, [x, y, time]: [number, number, number], active: boolean) => {
      const touch = { identifier: 3, clientX: x, clientY: y };
      const event = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperty(event, "touches", { value: active ? [touch] : [] });
      Object.defineProperty(event, "changedTouches", { value: [touch] });
      Object.defineProperty(event, "timeStamp", { value: time });
      target.dispatchEvent(event);
      return event;
    };
    touchEvent("touchstart", points[0]!, true);
    const moves = points.slice(1).map((point) => touchEvent("touchmove", point, true));
    const end = touchEvent("touchend", points[points.length - 1]!, false);
    return { moves, end };
  }

  /** 24 rows over 480px: 20px per line. */
  function giveScreenGeometry(term: (typeof terminalMocks.instances)[number]) {
    const screen = term.element.querySelector<HTMLElement>(".xterm-screen")!;
    screen.getBoundingClientRect = () => ({ left: 0, top: 0, right: 800, bottom: 480, width: 800, height: 480, x: 0, y: 0, toJSON() {} });
    return screen;
  }

  it("scrolls the scrollback by whole lines for a vertical drag without touching focus or the keyboard mode", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    term.buffer.active.viewportY = 100;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    const blur = vi.spyOn(term.textarea, "blur");
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    // Finger moves down 100px slowly (no fling): five lines toward older output.
    const { moves, end } = drag(host, [[50, 100, 0], [50, 115, 300], [50, 160, 600], [50, 215, 900]]);
    expect(term.scrollLines.mock.calls.map(([lines]) => lines)).toEqual([-2, -3]);
    expect(term.buffer.active.viewportY).toBe(95);
    expect(moves.slice(1).every((event) => event.defaultPrevented)).toBe(true);
    // A drag is not a tap: no compatibility mouse events, no keyboard request, and the
    // speculative `text` mode of touchstart is undone.
    expect(end.defaultPrevented).toBe(true);
    expect(blur).not.toHaveBeenCalled();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    wrapper.unmount();
  });

  it("keeps an open keyboard's input mode while scrolling", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    term.buffer.active.viewportY = 50;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    tap(host);
    term.textarea.focus();
    window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible: true } }));
    await flushPromises();
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    drag(host, [[50, 300, 0], [50, 260, 400], [50, 200, 800]]);
    expect(term.scrollLines).toHaveBeenCalled();
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    expect(document.activeElement).toBe(term.textarea);
    wrapper.unmount();
  });

  it("continues a fast release as a decaying fling and a new touch stops it without counting as a tap", async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
    vi.stubGlobal("cancelAnimationFrame", (id: number) => { frames[id - 1] = () => {}; });
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    term.buffer.active.viewportY = 400;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    drag(host, [[50, 100, 0], [50, 120, 10], [50, 220, 30], [50, 320, 50]]);
    const afterDrag = term.buffer.active.viewportY;
    const pending = frames.length;
    expect(pending).toBeGreaterThan(0);
    const start = performance.now();
    frames[pending - 1]!(start + 16);
    frames[frames.length - 1]!(start + 32);
    expect(term.buffer.active.viewportY).toBeLessThan(afterDrag);
    // The next finger only stops the fling: no keyboard request.
    const blur = vi.spyOn(term.textarea, "blur");
    term.textarea.focus();
    const scrolled = term.scrollLines.mock.calls.length;
    tap(host);
    expect(blur).not.toHaveBeenCalled();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    frames[frames.length - 1]!(start + 48);
    expect(term.scrollLines.mock.calls.length).toBe(scrolled);
    wrapper.unmount();
  });

  it("sends drags on the alternate screen and in mouse-reporting apps through xterm's wheel handling", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const screen = giveScreenGeometry(term);
    const wheels: WheelEvent[] = [];
    term.element.addEventListener("wheel", (event) => wheels.push(event as WheelEvent));
    const host = wrapper.find(".nvx-terminal-view__host").element;
    term.buffer.active.type = "alternate";
    // Finger up 60px: three lines toward newer output, i.e. wheel down.
    drag(host, [[120, 300, 0], [120, 289, 300], [120, 229, 600]]);
    expect(term.scrollLines).not.toHaveBeenCalled();
    expect(wheels.map((event) => [event.deltaY, event.deltaMode, event.target === screen])).toEqual([
      [1, WheelEvent.DOM_DELTA_LINE, true],
      [1, WheelEvent.DOM_DELTA_LINE, true],
      [1, WheelEvent.DOM_DELTA_LINE, true],
    ]);
    term.buffer.active.type = "normal";
    terminalMocks.mouseTrackingMode = "vt200";
    wheels.length = 0;
    drag(host, [[120, 240, 0], [120, 251, 300], [120, 291, 600]]);
    expect(wheels.map((event) => event.deltaY)).toEqual([-1, -1]);
    expect(term.scrollLines).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("ignores sideways drags", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    const host = wrapper.find(".nvx-terminal-view__host").element;
    const { end } = drag(host, [[50, 100, 0], [80, 104, 100], [200, 110, 200]]);
    expect(term.scrollLines).not.toHaveBeenCalled();
    expect(end.defaultPrevented).toBe(true);
    wrapper.unmount();
  });

  type TestTouch = { identifier: number; clientX: number; clientY: number };
  /** A touch event with explicit active and changed fingers, as the capture listeners see it. */
  function fingerEvent(target: Element, type: string, active: TestTouch[], changed: TestTouch[], time = 0) {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, "touches", { value: active });
    Object.defineProperty(event, "changedTouches", { value: changed });
    Object.defineProperty(event, "timeStamp", { value: time });
    target.dispatchEvent(event);
    return event;
  }

  function collectFrames() {
    const frames: Array<FrameRequestCallback | null> = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
    vi.stubGlobal("cancelAnimationFrame", (id: number) => { if (id > 0) frames[id - 1] = null; });
    return () => {
      for (let index = 0; index < frames.length; index += 1) {
        const frame = frames[index];
        frames[index] = null;
        frame?.(performance.now());
      }
    };
  }

  function mountMobileWithPinia(pinia: ReturnType<typeof createPinia>) {
    return mount(NvxTerminalView, {
      attachTo: document.body,
      props: { readOnly: false, terminalLabel: "Terminal", gapLabel: "Gap", paneId: "pane-1" },
      global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", missingWarn: false, fallbackWarn: false, messages: { en: { terminalEnhancements: { highlightSuspended: "" }, terminalInteraction: terminalInteractionEn } } })] },
    });
  }

  it("starts at the Android default size and scales it live with a pinch, persisting once every finger is up", async () => {
    const runFrames = collectFrames();
    const pinia = createPinia();
    const wrapper = mountMobileWithPinia(pinia);
    await flushPromises();
    const ui = useUiStore(pinia);
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    term.buffer.active.viewportY = 100;
    expect(ui.terminalFontSize).toBe(11);
    expect(term.options.fontSize).toBe(11);
    const host = wrapper.find(".nvx-terminal-view__host").element;
    Object.defineProperty(host, "clientWidth", { configurable: true, value: 800 });
    // Twice the font size halves the columns that fit, as FitAddon would measure.
    const fit = vi.spyOn(FitAddon.prototype, "fit").mockImplementation(() => { term.cols = 40; });
    const blur = vi.spyOn(term.textarea, "blur");
    const focus = vi.spyOn(term.textarea, "focus");
    const a = { identifier: 1, clientX: 100, clientY: 200 };
    const b = { identifier: 2, clientX: 100, clientY: 400 };
    fingerEvent(host, "touchstart", [a], [a]);
    // The first finger speculatively arms the keyboard mode; the second finger undoes it.
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    fingerEvent(host, "touchstart", [a, b], [b]);
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    const b2 = { ...b, clientY: 600 };
    const move = fingerEvent(host, "touchmove", [a, b2], [b2]);
    expect(move.defaultPrevented).toBe(true);
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-view__font-size-indicator").text()).toBe("Text size 22 px");
    // The preview is applied once per animation frame, then xterm refits.
    expect(term.options.fontSize).toBe(11);
    runFrames();
    expect(term.options.fontSize).toBe(22);
    expect(fit).toHaveBeenCalled();
    // The refit reaches the Pane through the ordinary resize event (and so the Core resize path).
    expect(wrapper.emitted("resize")?.at(-1)).toEqual([24, 40]);
    expect(ui.terminalFontSize).toBe(11);
    // Lifting one finger leaves the other inert: it neither scrolls nor taps.
    expect(fingerEvent(host, "touchend", [a], [b2]).defaultPrevented).toBe(true);
    const a2 = { ...a, clientY: 320 };
    expect(fingerEvent(host, "touchmove", [a2], [a2]).defaultPrevented).toBe(true);
    expect(term.scrollLines).not.toHaveBeenCalled();
    expect(fingerEvent(host, "touchend", [], [a2]).defaultPrevented).toBe(true);
    await flushPromises();
    expect(ui.terminalFontSize).toBe(22);
    expect(JSON.parse(localStorage.getItem("norishell.ui.preferences.v1") ?? "{}")).toMatchObject({ terminalFontSize: 22 });
    expect(wrapper.find(".nvx-terminal-view__font-size-indicator").exists()).toBe(false);
    // Long-press never fired, nothing was selected, no keyboard request or input.
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(wrapper.find(".nvx-terminal-view__touch-actions").exists()).toBe(false);
    expect(term.select).not.toHaveBeenCalled();
    expect(blur).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    expect(wrapper.emitted("input")).toBeUndefined();
    // A later single finger scrolls normally again.
    drag(host, [[50, 100, 0], [50, 115, 300], [50, 160, 600]]);
    expect(term.scrollLines).toHaveBeenCalled();
    fit.mockRestore();
    wrapper.unmount();
  });

  it("turns a running scroll into a pinch when a second finger lands, with no fling afterwards", async () => {
    const runFrames = collectFrames();
    const pinia = createPinia();
    const wrapper = mountMobileWithPinia(pinia);
    await flushPromises();
    const ui = useUiStore(pinia);
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    term.buffer.active.viewportY = 200;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    const a = { identifier: 1, clientX: 100, clientY: 100 };
    fingerEvent(host, "touchstart", [a], [a], 0);
    const a1 = { ...a, clientY: 140 };
    fingerEvent(host, "touchmove", [a1], [a1], 10);
    const a2 = { ...a, clientY: 240 };
    fingerEvent(host, "touchmove", [a2], [a2], 20);
    const scrolled = term.scrollLines.mock.calls.length;
    expect(scrolled).toBeGreaterThan(0);
    const b = { identifier: 2, clientX: 100, clientY: 440 };
    fingerEvent(host, "touchstart", [a2, b], [b], 30);
    // The pinch shrinks the span to half: 11px → 6 clamps to the 10px minimum.
    const b2 = { ...b, clientY: 340 };
    fingerEvent(host, "touchmove", [a2, b2], [b2], 40);
    runFrames();
    expect(term.options.fontSize).toBe(10);
    expect(term.scrollLines.mock.calls.length).toBe(scrolled);
    fingerEvent(host, "touchend", [b2], [a2], 50);
    fingerEvent(host, "touchend", [], [b2], 60);
    runFrames();
    expect(term.scrollLines.mock.calls.length).toBe(scrolled);
    await flushPromises();
    expect(ui.terminalFontSize).toBe(10);
    wrapper.unmount();
  });

  it("keeps the keyboard mode of fingers that land together and recovers from a gesture end it never saw", async () => {
    const runFrames = collectFrames();
    const pinia = createPinia();
    const wrapper = mountMobileWithPinia(pinia);
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    const host = wrapper.find(".nvx-terminal-view__host").element;
    // An earlier tap armed `text`; the keyboard was then dismissed.
    tap(host);
    window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible: false } }));
    await flushPromises();
    term.textarea.setAttribute("inputmode", "none");
    const a = { identifier: 1, clientX: 100, clientY: 200 };
    const b = { identifier: 2, clientX: 100, clientY: 300 };
    fingerEvent(host, "touchstart", [a, b], [a, b]);
    const b2 = { ...b, clientY: 400 };
    fingerEvent(host, "touchmove", [a, b2], [b2]);
    runFrames();
    expect(term.options.fontSize).toBe(22);
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    // The view misses the end of this gesture; a fresh single finger still scrolls.
    term.buffer.active.viewportY = 100;
    drag(host, [[50, 100, 0], [50, 115, 300], [50, 160, 600]]);
    expect(term.scrollLines).toHaveBeenCalled();
    wrapper.unmount();
  });

  it("restores the stored size and reports it when the pinch result cannot be saved", async () => {
    const runFrames = collectFrames();
    const pinia = createPinia();
    const wrapper = mountMobileWithPinia(pinia);
    await flushPromises();
    const ui = useUiStore(pinia);
    const tips = useTipsStore(pinia);
    const save = vi.spyOn(ui, "setTerminalFontSize").mockResolvedValue(false);
    const term = terminalMocks.instances[0]!;
    giveScreenGeometry(term);
    const host = wrapper.find(".nvx-terminal-view__host").element;
    const a = { identifier: 1, clientX: 100, clientY: 200 };
    const b = { identifier: 2, clientX: 100, clientY: 400 };
    fingerEvent(host, "touchstart", [a], [a]);
    fingerEvent(host, "touchstart", [a, b], [b]);
    const b2 = { ...b, clientY: 500 };
    fingerEvent(host, "touchmove", [a, b2], [b2]);
    runFrames();
    expect(term.options.fontSize).toBe(17);
    fingerEvent(host, "touchend", [], [a, b2]);
    await flushPromises();
    expect(save).toHaveBeenCalledWith(17);
    expect(ui.terminalFontSize).toBe(11);
    expect(term.options.fontSize).toBe(11);
    expect(tips.items.map((item) => item.title)).toContain(terminalInteractionEn.fontSizeSaveFailed);
    wrapper.unmount();
  });

  it("re-focuses the textarea to open the soft keyboard when a tap landed before the input lease arrived", async () => {
    const wrapper = mountMobile(true);
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    tap(host);
    // xterm focuses its (still readonly) textarea on the tap; Android shows no keyboard for it.
    term.textarea.focus();
    const blur = vi.spyOn(term.textarea, "blur");
    const focus = vi.spyOn(term.textarea, "focus");
    await wrapper.setProps({ readOnly: false });
    expect(blur).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    // The lease arriving again later (no new tap) must not pop the keyboard.
    await wrapper.setProps({ readOnly: true });
    await wrapper.setProps({ readOnly: false });
    expect(blur).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("keeps inputmode=none for programmatic focus and switches to text only for a touch on the terminal surface", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    // Mounted (and any programmatic activation focus): no keyboard.
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    (wrapper.vm as unknown as { focus(): void }).focus();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    // A finger on the terminal: the mousedown focus that follows may open the keyboard.
    tap(host);
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    // Focus moving to a dialog field or menu item arms "no keyboard" again for the restoration focus.
    term.textarea.focus();
    const other = document.createElement("button");
    document.body.append(other);
    other.focus();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    other.remove();
    // The Activity reporting the keyboard closed while the terminal keeps focus does the same.
    tap(host);
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible: true } }));
    await flushPromises();
    window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible: false } }));
    await flushPromises();
    expect(term.textarea.getAttribute("inputmode")).toBe("none");
    wrapper.unmount();
  });

  it("re-raises the keyboard for a tap on an already focused terminal while the keyboard is closed", async () => {
    const wrapper = mountMobile();
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    term.textarea.focus();
    const blur = vi.spyOn(term.textarea, "blur");
    const focus = vi.spyOn(term.textarea, "focus");
    tap(host);
    expect(blur).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledTimes(1);
    expect(term.textarea.getAttribute("inputmode")).toBe("text");
    // With the keyboard already open the same tap changes nothing.
    window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible: true } }));
    await flushPromises();
    tap(host);
    expect(blur).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("does not re-focus for a lease that arrives without a tap, after a tap on a writable view, or when focus moved away", async () => {
    const wrapper = mountMobile(true);
    await flushPromises();
    const term = terminalMocks.instances[0]!;
    const host = wrapper.find(".nvx-terminal-view__host").element;
    const blur = vi.spyOn(term.textarea, "blur");
    term.textarea.focus();
    // Programmatic activation (Tab switch, returning to the page): no tap, no keyboard.
    await wrapper.setProps({ readOnly: false });
    expect(blur).not.toHaveBeenCalled();
    // A tap on an already writable, focused view re-raises the keyboard once and remembers nothing:
    // later lease changes do not touch focus.
    tap(host);
    expect(blur).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ readOnly: true });
    await wrapper.setProps({ readOnly: false });
    expect(blur).toHaveBeenCalledTimes(1);
    // A tap while read-only whose focus has since moved elsewhere is not redirected to the terminal.
    await wrapper.setProps({ readOnly: true });
    tap(host);
    term.textarea.blur();
    blur.mockClear();
    await wrapper.setProps({ readOnly: false });
    expect(blur).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
