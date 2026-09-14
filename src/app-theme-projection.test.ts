import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BUILTIN_APP_THEMES } from "./app-theme";
import { initializeAuxiliaryThemeAppearance, publishAppThemeProjection, readAppThemeProjection } from "./app-theme-projection";
import { applySecureWindowAppearance } from "./secure-window";
import { UI_PREFERENCES_KEY } from "./ui-preferences";

const key = "norishell.app-theme.projection.v1";

describe("auxiliary theme projection", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("style");
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} })));
  });
  afterEach(() => vi.unstubAllGlobals());

  it("rejects corrupt, wrong-appearance and executable-shaped cached themes", () => {
    for (const value of ["{", JSON.stringify({ light: BUILTIN_APP_THEMES.dark }), JSON.stringify({ light: { ...BUILTIN_APP_THEMES.light, css: "body{}" } })]) {
      localStorage.setItem(key, value);
      expect(readAppThemeProjection("light")).toEqual(BUILTIN_APP_THEMES.light);
    }
  });

  it("updates the tray projection on storage changes and detaches listeners", () => {
    const light = { ...BUILTIN_APP_THEMES.light, radius: 10 };
    publishAppThemeProjection({ light, dark: BUILTIN_APP_THEMES.dark });
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "light" }));
    const dispose = initializeAuxiliaryThemeAppearance();
    expect(document.documentElement.style.getPropertyValue("--nvx-radius-md")).toBe("10px");
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "dark" }));
    window.dispatchEvent(new StorageEvent("storage", { key: UI_PREFERENCES_KEY }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--nvx-radius-md")).toBe("6px");
    dispose();
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "light" }));
    window.dispatchEvent(new StorageEvent("storage", { key: UI_PREFERENCES_KEY }));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("keeps secure windows independent of the custom theme projection", () => {
    publishAppThemeProjection({ light: { ...BUILTIN_APP_THEMES.light, radius: 10 }, dark: BUILTIN_APP_THEMES.dark });
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "light", locale: "en" }));
    applySecureWindowAppearance();
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.style.length).toBe(0);
  });
});
