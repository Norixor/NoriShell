/// <reference types="node" />

import { createPinia, setActivePinia } from "pinia";
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ listPluginThemes: vi.fn() }));
vi.mock("./core-api/app-theme", () => core);

import {
  APP_THEME_COLOR_KEYS,
  APP_THEME_COLOR_CSS_VARIABLES,
  BUILTIN_APP_THEMES,
  BUILTIN_DARK_THEME_ID,
  BUILTIN_LIGHT_THEME_ID,
  cloneDefaultAppThemeProfile,
  parseThemeDefinition,
  themeStyles,
  validateAppThemeProfile,
} from "./app-theme";
import { APP_THEME_PREFERENCES_KEY, useAppThemeStore } from "./stores/appTheme";

describe("application theme contract", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("style");
    setActivePinia(createPinia());
    core.listPluginThemes.mockReset();
  });

  it("validates complete, contrast-safe package definitions and rejects executable-shaped data", () => {
    expect(parseThemeDefinition(BUILTIN_APP_THEMES.light)).not.toBeNull();
    expect(parseThemeDefinition({ ...BUILTIN_APP_THEMES.light, colors: { ...BUILTIN_APP_THEMES.light.colors, textSecondary: "#ffffff" } })).toBeNull();
    expect(parseThemeDefinition({ ...BUILTIN_APP_THEMES.light, script: "alert(1)" })).toBeNull();
    expect(parseThemeDefinition({ ...BUILTIN_APP_THEMES.light, colors: { ...BUILTIN_APP_THEMES.light.colors, bogus: "#ffffff" } })).toBeNull();
    expect(parseThemeDefinition({ ...BUILTIN_APP_THEMES.light, id: BigInt(1) })).toBeNull();
    expect(APP_THEME_COLOR_KEYS).toHaveLength(26);
  });

  it("maps every bootstrap built-in token and default density without engine drift", () => {
    const styles = themeStyles(BUILTIN_APP_THEMES.light);
    const css = readFileSync(`${process.cwd()}/src/styles/tokens.css`, "utf8");
    const lightBlock = css.slice(css.indexOf(":root,"), css.indexOf("[data-theme=\"dark\"]"));
    for (const colorKey of APP_THEME_COLOR_KEYS) {
      const variable = APP_THEME_COLOR_CSS_VARIABLES[colorKey];
      expect(styles[variable]).toBe(BUILTIN_APP_THEMES.light.colors[colorKey]);
      expect(lightBlock).toContain(`${variable}: ${BUILTIN_APP_THEMES.light.colors[colorKey]};`);
    }
    expect(styles).toMatchObject({
      "--nvx-radius-sm": "4px", "--nvx-radius-md": "6px",
      "--nvx-space-4": "16px", "--nvx-control-height-md": "40px",
      "--nvx-line-height-body": "22px",
    });
  });

  it("pins external choices to package plugin identity and falls back when disabled", async () => {
    const definition = { ...BUILTIN_APP_THEMES.dark, id: "midnight", name: { zhCN: "午夜", en: "Midnight" } };
    core.listPluginThemes.mockResolvedValue({ themes: [{
      pluginId: "org.norishell.midnight", packageHash: "a".repeat(64), version: "1.0.0", enabled: false, definition,
    }] });
    const store = useAppThemeStore();
    await store.refreshThemes();
    expect(store.themes.find((item) => item.source === "plugin")?.key).toBe("plugin:org.norishell.midnight:midnight");
    expect(store.themes.find((item) => item.source === "plugin")?.enabled).toBe(false);
    const profile = cloneDefaultAppThemeProfile();
    profile.darkThemeId = "plugin:org.norishell.midnight:midnight";
    expect(await store.saveProfile(profile)).toBe(true);
    expect(store.resolveTheme("dark").id).toBe("norishell-dark");
  });

  it("never imports an embedded external definition and updates CSS only after persistence", async () => {
    const store = useAppThemeStore();
    const profile = cloneDefaultAppThemeProfile();
    profile.overrides[BUILTIN_LIGHT_THEME_ID] = { colors: { accent: "#123456" }, fontSize: 15 };
    expect(await store.saveProfile(profile)).toBe(true);
    store.applyTheme("light");
    expect(document.documentElement.style.getPropertyValue("--nvx-color-accent")).toBe("#123456");
    expect(document.documentElement.style.getPropertyValue("--nvx-font-size-body")).toBe("15px");
    expect(JSON.parse(store.exportProfile())).toEqual(profile);
    expect(validateAppThemeProfile({ ...profile, definition: BUILTIN_APP_THEMES.light })).toBe(false);

    const previous = store.appThemePreferences();
    vi.spyOn(localStorage, "setItem").mockImplementationOnce(() => { throw new Error("quota"); });
    expect(await store.saveProfile({ ...profile, lightThemeId: BUILTIN_DARK_THEME_ID })).toBe(false);
    expect(store.appThemePreferences()).toEqual(previous);
    expect(JSON.parse(localStorage.getItem(APP_THEME_PREFERENCES_KEY) ?? "null")).toEqual(previous);
  });

  it("rejects a saved unreadable selection and resolves stale package overrides to their verified base", async () => {
    const store = useAppThemeStore();
    const profile = cloneDefaultAppThemeProfile();
    profile.overrides[BUILTIN_LIGHT_THEME_ID] = { colors: { onAccent: "#123456" } };
    expect(await store.saveProfile(profile)).toBe(false);
    expect(store.resolveTheme("light", profile).colors.onAccent).toBe(BUILTIN_APP_THEMES.light.colors.onAccent);
  });

  it("keeps built-ins on a failed Core read and only maps host-owned style properties", async () => {
    core.listPluginThemes.mockRejectedValue(new Error("unavailable"));
    const store = useAppThemeStore();
    await store.refreshThemes();
    expect(store.themes.map((item) => item.key)).toEqual([BUILTIN_LIGHT_THEME_ID, BUILTIN_DARK_THEME_ID]);
    expect(store.loadError).toBe("unavailable");
    expect(Object.keys(themeStyles(BUILTIN_APP_THEMES.dark))).toContain("--nvx-color-bg-canvas");
    expect(Object.keys(themeStyles(BUILTIN_APP_THEMES.dark))).not.toContain("background-image");
  });

  it("ignores a late refresh response so removed packages cannot reappear", async () => {
    let finishFirst: ((value: unknown) => void) | undefined;
    const first = new Promise<unknown>((resolve) => { finishFirst = resolve; });
    core.listPluginThemes
      .mockReturnValueOnce(first)
      .mockResolvedValueOnce({ themes: [{
        pluginId: "org.norishell.clear", packageHash: "c".repeat(64), version: "1.0.0", enabled: true,
        definition: { ...BUILTIN_APP_THEMES.light, id: "clear", name: { zhCN: "清朗", en: "Clear" } },
      }] });
    const store = useAppThemeStore();
    const stale = store.refreshThemes();
    await store.refreshThemes();
    finishFirst?.({ themes: [{
      pluginId: "org.norishell.midnight", packageHash: "d".repeat(64), version: "1.0.0", enabled: true,
      definition: { ...BUILTIN_APP_THEMES.dark, id: "midnight", name: { zhCN: "午夜", en: "Midnight" } },
    }] });
    await stale;
    expect(store.themes.map((item) => item.key)).toEqual([BUILTIN_LIGHT_THEME_ID, BUILTIN_DARK_THEME_ID, "plugin:org.norishell.clear:clear"]);
    expect(store.loading).toBe(false);
  });

  it("keeps an override after a package update makes it unreadable, while resolving the verified package base", async () => {
    const initial = {
      ...BUILTIN_APP_THEMES.light, id: "clear", name: { zhCN: "清朗", en: "Clear" },
    };
    const updated = {
      ...initial,
      colors: { ...initial.colors, accent: "#f0f0f0", accentHover: "#e0e0e0", onAccent: "#101217" },
    };
    const entry = (definition: typeof initial) => ({
      pluginId: "org.norishell.clear", packageHash: "e".repeat(64), version: "1.0.0", enabled: true, definition,
    });
    core.listPluginThemes.mockResolvedValueOnce({ themes: [entry(initial)] }).mockResolvedValueOnce({ themes: [entry(updated)] });
    const store = useAppThemeStore();
    await store.refreshThemes();
    const profile = cloneDefaultAppThemeProfile();
    profile.lightThemeId = "plugin:org.norishell.clear:clear";
    profile.overrides[profile.lightThemeId] = { colors: { accent: "#123456" } };
    expect(await store.saveProfile(profile)).toBe(true);

    await store.refreshThemes();
    expect(store.resolveTheme("light").colors.accent).toBe("#f0f0f0");
    expect(JSON.parse(store.exportProfile()).overrides[profile.lightThemeId].colors.accent).toBe("#123456");
  });
});
