import { defineStore } from "pinia";
import { computed, ref } from "vue";

import {
  BUILTIN_APP_THEMES,
  BUILTIN_DARK_THEME_ID,
  BUILTIN_LIGHT_THEME_ID,
  cloneAppThemeProfile,
  cloneDefaultAppThemeProfile,
  parseThemeDefinition,
  pluginThemeKey,
  themeStyles,
  themeWithOverride,
  validateAppThemeProfile,
  validateThemeDefinition,
  type AppThemeProfile,
  type ThemeAppearance,
  type ThemeChoiceKey,
  type ThemeDefinition,
} from "../app-theme";
import { publishAppThemeProjection } from "../app-theme-projection";
import { listPluginThemes, type ThemePackageEntry } from "../core-api/app-theme";

export const APP_THEME_PREFERENCES_KEY = "norishell.app-theme.v1";

export interface ThemeChoice {
  key: ThemeChoiceKey;
  source: "builtin" | "plugin";
  pluginId: string | null;
  packageHash: string | null;
  version: string | null;
  enabled: boolean;
  definition: ThemeDefinition;
}

function readProfile(): AppThemeProfile {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(APP_THEME_PREFERENCES_KEY) ?? "null");
    return validateAppThemeProfile(stored) ? cloneAppThemeProfile(stored) : cloneDefaultAppThemeProfile();
  } catch {
    return cloneDefaultAppThemeProfile();
  }
}

function cloneChoice(choice: ThemeChoice): ThemeChoice {
  return { ...choice, definition: parseThemeDefinition(choice.definition)! };
}

function builtins(): ThemeChoice[] {
  const choices: ThemeChoice[] = [
    { key: BUILTIN_LIGHT_THEME_ID, source: "builtin", pluginId: null, packageHash: null, version: null, enabled: true, definition: BUILTIN_APP_THEMES.light },
    { key: BUILTIN_DARK_THEME_ID, source: "builtin", pluginId: null, packageHash: null, version: null, enabled: true, definition: BUILTIN_APP_THEMES.dark },
  ];
  return choices.map(cloneChoice);
}

function validPackageEntry(value: unknown): value is ThemePackageEntry {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const entry = value as Record<string, unknown>;
  return Object.keys(entry).length === 5
    && ["pluginId", "packageHash", "version", "enabled", "definition"].every((key) => key in entry)
    && typeof entry.pluginId === "string" && entry.pluginId.length > 0 && entry.pluginId.length <= 160
    && typeof entry.packageHash === "string" && /^[a-f0-9]{64}$/u.test(entry.packageHash)
    && typeof entry.version === "string" && entry.version.length > 0 && entry.version.length <= 80
    && typeof entry.enabled === "boolean" && parseThemeDefinition(entry.definition) !== null;
}

function choicesFromPackages(value: unknown): ThemeChoice[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const response = value as Record<string, unknown>;
  if (Object.keys(response).length !== 1 || !Array.isArray(response.themes) || response.themes.length > 128) return [];
  const seen = new Set<string>();
  const choices: ThemeChoice[] = [];
  for (const entry of response.themes) {
    if (!validPackageEntry(entry)) continue;
    const definition = parseThemeDefinition(entry.definition);
    if (!definition || seen.has(pluginThemeKey(entry.pluginId, definition.id))) continue;
    seen.add(pluginThemeKey(entry.pluginId, definition.id));
    choices.push({
      key: pluginThemeKey(entry.pluginId, definition.id), source: "plugin", pluginId: entry.pluginId,
      packageHash: entry.packageHash, version: entry.version, enabled: entry.enabled, definition,
    });
  }
  return choices;
}

export const useAppThemeStore = defineStore("appTheme", () => {
  const themeChoices = ref<ThemeChoice[]>(builtins());
  function resolvedChoice(appearance: ThemeAppearance, requestedProfile: AppThemeProfile, choices = themeChoices.value): ThemeChoice {
    const selected = appearance === "light" ? requestedProfile.lightThemeId : requestedProfile.darkThemeId;
    return choices.find((item) => item.key === selected && item.enabled && item.definition.appearance === appearance)
      ?? choices.find((item) => item.key === (appearance === "light" ? BUILTIN_LIGHT_THEME_ID : BUILTIN_DARK_THEME_ID))!;
  }

  function isSafeOverride(choice: ThemeChoice, requestedProfile: AppThemeProfile) {
    return validateThemeDefinition(themeWithOverride(choice.definition, requestedProfile.overrides[choice.key]));
  }

  /** Stored user overrides survive package updates, disablement, and removal. Resolution validates them separately. */
  function normalizedProfile(value: AppThemeProfile): AppThemeProfile {
    return cloneAppThemeProfile(value);
  }

  function selectedDefinitionsAreSafe(value: AppThemeProfile) {
    return (["light", "dark"] as const).every((appearance) => isSafeOverride(resolvedChoice(appearance, value), value));
  }

  const profile = ref<AppThemeProfile>(normalizedProfile(readProfile()));
  const loading = ref(false);
  const loadError = ref<string | null>(null);
  const activeAppearance = ref<ThemeAppearance>("light");
  let refreshGeneration = 0;

  const themes = computed(() => themeChoices.value.map(cloneChoice));

  function resolveTheme(appearance: ThemeAppearance, requestedProfile = profile.value): ThemeDefinition {
    const choice = resolvedChoice(appearance, requestedProfile);
    const resolved = themeWithOverride(choice.definition, requestedProfile.overrides[choice.key]);
    // An imported override can become invalid after a package update. Never apply it to the DOM.
    return validateThemeDefinition(resolved) ? resolved : choice.definition;
  }

  const resolvedTheme = computed(() => resolveTheme(activeAppearance.value));

  function applyTheme(appearance: ThemeAppearance): ThemeDefinition {
    activeAppearance.value = appearance;
    const resolved = resolveTheme(appearance);
    for (const [property, value] of Object.entries(themeStyles(resolved))) {
      document.documentElement.style.setProperty(property, value);
    }
    publishAppThemeProjection({ light: resolveTheme("light"), dark: resolveTheme("dark") });
    return resolved;
  }

  function saveProfile(next: unknown, expectedProfile: AppThemeProfile | null = null): boolean {
    if (!validateAppThemeProfile(next)
      || (expectedProfile !== null && JSON.stringify(profile.value) !== JSON.stringify(expectedProfile))) return false;
    const saved = cloneAppThemeProfile(next);
    if (!selectedDefinitionsAreSafe(saved)) return false;
    try {
      localStorage.setItem(APP_THEME_PREFERENCES_KEY, JSON.stringify(saved));
    } catch {
      return false;
    }
    profile.value = saved;
    applyTheme(activeAppearance.value);
    return true;
  }

  function appThemePreferences(): AppThemeProfile { return cloneAppThemeProfile(profile.value); }
  function replaceAppThemePreferences(next: unknown, expected: AppThemeProfile): boolean {
    return saveProfile(next, expected);
  }

  function exportProfile(): string { return JSON.stringify(appThemePreferences(), null, 2); }
  function importProfile(text: string): { ok: boolean; error?: "invalidFile" | "tooLarge" | "persistFailed" } {
    if (new TextEncoder().encode(text).byteLength > 32 * 1024) return { ok: false, error: "tooLarge" };
    try {
      const parsed: unknown = JSON.parse(text);
      if (!validateAppThemeProfile(parsed)) return { ok: false, error: "invalidFile" };
      return saveProfile(parsed) ? { ok: true } : { ok: false, error: "persistFailed" };
    } catch { return { ok: false, error: "invalidFile" }; }
  }

  async function refreshThemes(): Promise<void> {
    const generation = ++refreshGeneration;
    loading.value = true;
    try {
      const response = await listPluginThemes();
      if (generation !== refreshGeneration) return;
      themeChoices.value = [...builtins(), ...choicesFromPackages(response)];
      profile.value = normalizedProfile(profile.value);
      loadError.value = null;
      applyTheme(activeAppearance.value);
    } catch {
      if (generation !== refreshGeneration) return;
      // A failed Core read never removes the safe built-in fallback or installs data from an old response.
      themeChoices.value = builtins();
      profile.value = normalizedProfile(profile.value);
      loadError.value = "unavailable";
      applyTheme(activeAppearance.value);
    } finally {
      if (generation === refreshGeneration) loading.value = false;
    }
  }

  return {
    profile, themes, loading, loadError, resolvedTheme,
    refreshThemes, resolveTheme, saveProfile, appThemePreferences, replaceAppThemePreferences,
    exportProfile, importProfile, applyTheme,
  };
});
