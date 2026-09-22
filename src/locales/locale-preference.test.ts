import { createPinia, setActivePinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";
import { resolveLocale } from "./index";
import { useUiStore } from "../stores/ui";
import { UI_PREFERENCES_KEY } from "../ui-preferences";
import { DEFAULT_APPLICATION_PREFERENCES, validateApplicationPreferences } from "../ui-transfer";

afterEach(() => vi.restoreAllMocks());

describe("system language preference", () => {
  it("resolves supported system languages in preference order and falls back to English", () => {
    expect(resolveLocale("system", ["fr-FR", "zh-TW", "en-US"])).toBe("zh-CN");
    expect(resolveLocale("system", ["en-GB", "zh-CN"])).toBe("en");
    expect(resolveLocale("system", ["ja-JP"])).toBe("en");
    expect(resolveLocale("zh-CN", ["en-US"])).toBe("zh-CN");
    expect(DEFAULT_APPLICATION_PREFERENCES.locale).toBe("system");
    expect(validateApplicationPreferences(DEFAULT_APPLICATION_PREFERENCES)).toBe(true);
  });

  it("follows language changes without replacing the saved preference or a manual override", () => {
    localStorage.clear();
    setActivePinia(createPinia());
    const languages = vi.spyOn(navigator, "languages", "get").mockReturnValue(["zh-CN"]);
    const store = useUiStore();
    expect(store.localePreference).toBe("system");
    expect(store.locale).toBe("zh-CN");
    languages.mockReturnValue(["en-US"]);
    window.dispatchEvent(new Event("languagechange"));
    expect(store.locale).toBe("en");
    expect(document.documentElement.lang).toBe("en");
    store.setLocale("zh-CN");
    window.dispatchEvent(new Event("languagechange"));
    expect(store.locale).toBe("zh-CN");
    store.setLocale("system");
    expect(store.applicationPreferences().locale).toBe("system");
    expect(JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY)!).locale).toBe("system");
    store.$dispose();
  });

  it("retains an existing explicit language selection", () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ locale: "zh-CN" }));
    setActivePinia(createPinia());
    vi.spyOn(navigator, "languages", "get").mockReturnValue(["en-US"]);
    const store = useUiStore();
    expect(store.localePreference).toBe("zh-CN");
    expect(store.locale).toBe("zh-CN");
    store.$dispose();
  });
});
