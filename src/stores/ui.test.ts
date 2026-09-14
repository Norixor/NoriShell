import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { UI_PREFERENCES_KEY } from "../ui-preferences";
import { useUiStore } from "./ui";

function stubSystemTheme(initiallyDark = false) {
  let matches = initiallyDark;
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

  return {
    mediaQuery,
    setMatches(value: boolean) {
      matches = value;
      for (const listener of listeners) {
        listener({ matches, media: mediaQuery.media } as MediaQueryListEvent);
      }
    },
  };
}

describe("application theme preference", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.dataset.theme = "light";
    setActivePinia(createPinia());
  });

  afterEach(() => vi.unstubAllGlobals());

  it("keeps the resolved Theme contract while following system changes", () => {
    const systemTheme = stubSystemTheme(false);
    const store = useUiStore();

    expect(store.setThemePreference("system")).toBe(true);
    store.setTerminalThemeMode("nord");
    const fixedTerminalPalette = store.resolvedTerminalPalette;
    expect(store.themePreference).toBe("system");
    expect(store.theme).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY) ?? "{}")).toMatchObject({
      themePreference: "system",
      theme: "light",
    });

    systemTheme.setMatches(true);
    expect(store.theme).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(store.resolvedTerminalPalette).toBe(fixedTerminalPalette);

    expect(store.setTheme("light")).toBe(true);
    systemTheme.setMatches(false);
    expect(store.themePreference).toBe("light");
    expect(store.theme).toBe("light");
    expect(systemTheme.mediaQuery.removeEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );
    store.$dispose();
  });

  it("migrates legacy fixed themes and leaves the effective theme unchanged when saving fails", () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ theme: "dark" }));
    const store = useUiStore();
    expect(store.themePreference).toBe("dark");
    expect(store.theme).toBe("dark");
    document.documentElement.dataset.theme = "dark";
    const setItem = vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });

    expect(store.setThemePreference("light")).toBe(false);
    expect(store.themePreference).toBe("dark");
    expect(store.theme).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    setItem.mockRestore();
    store.$dispose();
  });

  it.each(["not json", "null", "[]"])("falls back to the existing light default for %s", (storedValue) => {
    localStorage.setItem(UI_PREFERENCES_KEY, storedValue);
    const store = useUiStore();

    expect(store.themePreference).toBe("light");
    expect(store.theme).toBe("light");
    store.$dispose();
  });
});
