import { afterEach, describe, expect, it, vi } from "vitest";

import { i18n } from "./locales";
import { applySecureWindowAppearance } from "./secure-window";
import { UI_PREFERENCES_KEY } from "./ui-preferences";

afterEach(() => {
  localStorage.removeItem(UI_PREFERENCES_KEY);
  applySecureWindowAppearance();
  vi.unstubAllGlobals();
});

describe("secure window appearance", () => {
  it("inherits only supported language and theme values from application preferences", () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ locale: "en", theme: "dark" }));
    applySecureWindowAppearance();
    expect(i18n.global.t("window.protected")).toBe("Protected window");
    expect(document.documentElement.lang).toBe("en");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it.each(["null", "invalid json", '{"locale":"unknown","theme":"unknown"}'])("falls back safely for %s", (value) => {
    localStorage.setItem(UI_PREFERENCES_KEY, value);
    applySecureWindowAppearance();
    expect(i18n.global.t("window.protected")).toBe("受保护的窗口");
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("follows system appearance only while the controlled preference requests it", () => {
    let matches = false;
    const listeners = new Set<(event: MediaQueryListEvent) => void>();
    const mediaQuery = {
      get matches() { return matches; },
      media: "(prefers-color-scheme: dark)",
      addEventListener: vi.fn((_type: string, listener: EventListenerOrEventListenerObject) => {
        if (typeof listener === "function") listeners.add(listener as (event: MediaQueryListEvent) => void);
      }),
      removeEventListener: vi.fn((_type: string, listener: EventListenerOrEventListenerObject) => {
        if (typeof listener === "function") listeners.delete(listener as (event: MediaQueryListEvent) => void);
      }),
    } as unknown as MediaQueryList;
    vi.stubGlobal("matchMedia", vi.fn(() => mediaQuery));
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "system" }));

    applySecureWindowAppearance();
    expect(document.documentElement.dataset.theme).toBe("light");
    matches = true;
    for (const listener of listeners) listener({ matches, media: mediaQuery.media } as MediaQueryListEvent);
    expect(document.documentElement.dataset.theme).toBe("dark");

    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "light" }));
    applySecureWindowAppearance();
    matches = false;
    for (const listener of listeners) listener({ matches, media: mediaQuery.media } as MediaQueryListEvent);
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(mediaQuery.removeEventListener).toHaveBeenCalledWith("change", expect.any(Function));
  });
});
