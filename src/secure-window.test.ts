import { afterEach, describe, expect, it, vi } from "vitest";

const zoom = vi.hoisted(() => ({ apply: vi.fn(async () => undefined) }));
vi.mock("./ui-zoom", async (original) => ({ ...await original<typeof import("./ui-zoom")>(), applyUiZoom: zoom.apply }));

import { i18n, resolveLocale } from "./locales";
import { applySecureWindowAppearance, initializeSecondaryWindowZoom, secondaryWindowZoom } from "./secure-window";
import { SECURE_WINDOW_APPEARANCE_KEY } from "./secure-window-appearance";
import { UI_PREFERENCES_KEY } from "./ui-preferences";

afterEach(() => {
  localStorage.removeItem(UI_PREFERENCES_KEY);
  localStorage.removeItem(SECURE_WINDOW_APPEARANCE_KEY);
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
    expect(document.documentElement.style.getPropertyValue("--nvx-layout-header-height")).toBe("");
  });

  it.each(["null", "invalid json", '{"locale":"unknown","theme":"unknown"}'])("falls back safely for %s", (value) => {
    localStorage.setItem(UI_PREFERENCES_KEY, value);
    applySecureWindowAppearance();
    expect(i18n.global.locale.value).toBe(resolveLocale("system"));
    expect(document.documentElement.lang).toBe(resolveLocale("system"));
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

describe("secondary window zoom", () => {
  it("follows the published UI zoom at startup and when it changes", async () => {
    zoom.apply.mockClear();
    localStorage.setItem(SECURE_WINDOW_APPEARANCE_KEY, JSON.stringify({ locale: "en", themePreference: "light", uiZoom: 90 }));
    initializeSecondaryWindowZoom();
    expect(zoom.apply).toHaveBeenLastCalledWith(90);
    // The Header frame uses this scale to keep native macOS controls inside their inset.
    await vi.waitFor(() => expect(secondaryWindowZoom.value).toBe(0.9));
    localStorage.setItem(SECURE_WINDOW_APPEARANCE_KEY, JSON.stringify({ locale: "en", themePreference: "light", uiZoom: 125 }));
    window.dispatchEvent(new StorageEvent("storage", { key: SECURE_WINDOW_APPEARANCE_KEY }));
    expect(zoom.apply).toHaveBeenLastCalledWith(125);
    expect(zoom.apply).toHaveBeenCalledTimes(2);
    window.dispatchEvent(new Event("pagehide"));
  });

  it("keeps the default scale when no valid zoom is stored", () => {
    zoom.apply.mockClear();
    localStorage.setItem(SECURE_WINDOW_APPEARANCE_KEY, JSON.stringify({ uiZoom: 33 }));
    initializeSecondaryWindowZoom();
    expect(zoom.apply).not.toHaveBeenCalled();
    window.dispatchEvent(new Event("pagehide"));
  });
});
