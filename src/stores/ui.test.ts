import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { UI_PREFERENCES_KEY } from "../ui-preferences";
import { DEFAULT_APPLICATION_PREFERENCES, validateApplicationPreferences } from "../ui-transfer";
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

  it("keeps the resolved Theme contract while following system changes", async () => {
    const systemTheme = stubSystemTheme(false);
    const store = useUiStore();

    expect(await store.setThemePreference("system")).toBe(true);
    await store.setTerminalThemeMode("nord");
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

    expect(await store.setTheme("light")).toBe(true);
    systemTheme.setMatches(false);
    expect(store.themePreference).toBe("light");
    expect(store.theme).toBe("light");
    expect(systemTheme.mediaQuery.removeEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );
    store.$dispose();
  });

  it("migrates legacy fixed themes and leaves the effective theme unchanged when saving fails", async () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ theme: "dark" }));
    const store = useUiStore();
    expect(store.themePreference).toBe("dark");
    expect(store.theme).toBe("dark");
    document.documentElement.dataset.theme = "dark";
    const setItem = vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });

    expect(await store.setThemePreference("light")).toBe(false);
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

describe("after-startup preference", () => {
  beforeEach(() => localStorage.clear());

  it("defaults never-set preferences to Overview and keeps any stored choice", () => {
    setActivePinia(createPinia());
    expect(useUiStore().terminalStartupBehavior).toBe("overview");
    expect(DEFAULT_APPLICATION_PREFERENCES.terminalStartupBehavior).toBe("overview");

    for (const stored of ["welcome", "restoreHistory", "overview"] as const) {
      localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ terminalStartupBehavior: stored }));
      setActivePinia(createPinia());
      const store = useUiStore();
      expect(store.terminalStartupBehavior).toBe(stored);
      expect(store.legacyApplicationPreferences().terminalStartupBehavior).toBe(stored);
      store.$dispose();
    }

    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "dark" }));
    setActivePinia(createPinia());
    expect(useUiStore().terminalStartupBehavior).toBe("overview");
  });

  it("persists the Overview choice and accepts older exports while rejecting unknown values", async () => {
    setActivePinia(createPinia());
    const store = useUiStore();
    expect(await store.setTerminalStartupBehavior("welcome")).toBe(true);
    expect(await store.setTerminalStartupBehavior("overview")).toBe(true);
    expect(JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY)!).terminalStartupBehavior).toBe("overview");

    for (const behavior of ["overview", "welcome", "restoreHistory"]) {
      expect(validateApplicationPreferences({ ...DEFAULT_APPLICATION_PREFERENCES, terminalStartupBehavior: behavior })).toBe(true);
    }
    for (const behavior of ["dashboard", "Overview", null, undefined]) {
      expect(validateApplicationPreferences({ ...DEFAULT_APPLICATION_PREFERENCES, terminalStartupBehavior: behavior })).toBe(false);
    }
    store.$dispose();
  });
});

describe("local terminal shell preference", () => {
  beforeEach(() => localStorage.clear());

  it("defaults to the system shell, persists a valid choice and falls back for invalid input", async () => {
    setActivePinia(createPinia());
    const store = useUiStore();
    expect(store.localTerminalShell).toBe("default");
    expect(await store.setLocalTerminalShell("path:/bin/bash")).toBe(true);
    expect(store.localTerminalShell).toBe("path:/bin/bash");
    expect(JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY)!).localTerminalShell).toBe("path:/bin/bash");

    setActivePinia(createPinia());
    expect(useUiStore().localTerminalShell).toBe("path:/bin/bash");

    expect(await store.setLocalTerminalShell("not a shell")).toBe(true);
    expect(store.localTerminalShell).toBe("default");
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ localTerminalShell: "path:" }));
    setActivePinia(createPinia());
    expect(useUiStore().localTerminalShell).toBe("default");
  });

  it("treats a Core snapshot without the field as the system default and always writes it back", () => {
    setActivePinia(createPinia());
    const store = useUiStore();
    store.hydrateCorePreferences({ ...DEFAULT_APPLICATION_PREFERENCES, localTerminalShell: "cmd" }, store.appearancePreferences());
    expect(store.localTerminalShell).toBe("cmd");
    const legacy: Record<string, unknown> = { ...DEFAULT_APPLICATION_PREFERENCES };
    delete legacy.localTerminalShell;
    store.hydrateCorePreferences(legacy as never, store.appearancePreferences());
    expect(store.localTerminalShell).toBe("default");
    expect(store.applicationPreferences().localTerminalShell).toBe("default");
  });
});
