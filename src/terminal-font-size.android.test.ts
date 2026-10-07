import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mobile = { value: true };
vi.mock("./platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./platform-host")>()),
  isMobileLayout: () => mobile.value,
}));

const {
  DESKTOP_DEFAULT_TERMINAL_FONT_SIZE,
  MOBILE_DEFAULT_TERMINAL_FONT_SIZE,
  defaultTerminalFontSize,
  parseTerminalFontSize,
} = await import("./terminal-theme");
const { defaultAppearancePreferences, validateAppearancePreferences } = await import("./ui-transfer");
const { useUiStore } = await import("./stores/ui");
const { UI_PREFERENCES_KEY } = await import("./ui-preferences");

describe("platform default terminal font size", () => {
  beforeEach(() => {
    mobile.value = true;
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it("is 11 on Android and 13 on desktop, from one source", () => {
    expect(MOBILE_DEFAULT_TERMINAL_FONT_SIZE).toBe(11);
    expect(defaultTerminalFontSize()).toBe(11);
    expect(parseTerminalFontSize(undefined)).toBe(11);
    expect(parseTerminalFontSize(9)).toBe(11);
    expect(defaultAppearancePreferences().terminalFontSize).toBe(11);
    mobile.value = false;
    expect(DESKTOP_DEFAULT_TERMINAL_FONT_SIZE).toBe(13);
    expect(defaultTerminalFontSize()).toBe(13);
    expect(parseTerminalFontSize(undefined)).toBe(13);
    expect(defaultAppearancePreferences().terminalFontSize).toBe(13);
  });

  it("starts a fresh Android store at 11 and keeps it within the shared Core range", () => {
    const ui = useUiStore();
    expect(ui.terminalFontSize).toBe(11);
    // The value written to Core on first launch passes the shared 10–28 validation unchanged.
    expect(validateAppearancePreferences(ui.appearancePreferences())).toBe(true);
    expect(ui.appearancePreferences().terminalFontSize).toBe(11);
  });

  it("never overrides a stored size, including an explicit 13", () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ terminalFontSize: 13 }));
    expect(useUiStore().terminalFontSize).toBe(13);
    setActivePinia(createPinia());
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ terminalFontSize: 16 }));
    expect(useUiStore().terminalFontSize).toBe(16);
  });

  it("keeps the desktop default for a fresh desktop store", () => {
    mobile.value = false;
    expect(useUiStore().terminalFontSize).toBe(13);
  });
});
