import { defineStore } from "pinia";
import { computed, onScopeDispose, ref, watch } from "vue";

import { useAppThemeStore } from "./appTheme";
import { cloneDefaultAppThemeProfile } from "../app-theme";
import { corePreferencesEnabled, saveApplicationPreferences } from "../core-api/application-preferences";
import { parseCoreApiError } from "../core-api/client";
import { publishSecureWindowAppearance } from "../secure-window-appearance";
import { validateApplicationPreferences, validateAppearancePreferences, type ApplicationPreferences, type AppearancePreferences } from "../ui-transfer";
import { applyUiZoom, isUiZoom, type UiZoom } from "../ui-zoom";
import { i18n, resolveLocale, type LocalePreference } from "../locales";
import {
  parseThemePreference,
  resolveThemePreference,
  UI_PREFERENCES_KEY,
  type ThemePreference,
} from "../ui-preferences";
import {
  cloneTerminalPalette,
  DEFAULT_TERMINAL_FONT_FAMILY,
  DEFAULT_TERMINAL_BOLD_FONT_WEIGHT,
  DARK_TERMINAL_PALETTE,
  isHexColor,
  LIGHT_TERMINAL_PALETTE,
  normalizeCustomTerminalPaletteName,
  normalizeTerminalFontFamily,
  parseTerminalPalette,
  parseTerminalFontSize,
  parseTerminalFontWeight,
  parseTerminalLineHeight,
  parseTerminalLetterSpacing,
  parseTerminalCursorStyle,
  parseStoredTerminalAppearance,
  resolveTerminalPalette,
  TERMINAL_PALETTE_CSS_VARIABLES,
  type TerminalColorKey,
  type TerminalCursorStyle,
  type TerminalPalette,
  type TerminalThemeMode,
} from "../terminal-theme";

export type Theme = "light" | "dark";
export type { ThemePreference } from "../ui-preferences";
export type TerminalStartupBehavior = "welcome" | "restoreHistory";
export type NewTerminalBehavior = "welcome" | "localTerminal";
export type SinglePaneTabCloseBehavior = "confirm" | "closeDirectly";



interface StoredUiPreferences {
  terminalAppearanceVersion?: unknown;
  theme?: unknown;
  themePreference?: unknown;
  uiZoom?: unknown;
  locale?: unknown;
  terminalStartupBehavior?: unknown;
  newTerminalBehavior?: unknown;
  singlePaneTabCloseBehavior?: unknown;
  terminalThemeMode?: unknown;
  terminalFontFamily?: unknown;
  terminalFontSize?: unknown;
  terminalFontWeight?: unknown;
  terminalBoldFontWeight?: unknown;
  terminalLineHeight?: unknown;
  terminalLetterSpacing?: unknown;
  terminalCursorStyle?: unknown;
  terminalCursorBlink?: unknown;
  customTerminalPalette?: unknown;
  customTerminalPaletteName?: unknown;
}

function readPreferences(): StoredUiPreferences {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return parsed as StoredUiPreferences;
  } catch {
    return {};
  }
}

// Visual fixtures are intentionally isolated from a user's persisted language choice.
// They are only available in development builds and exist for shareable product screenshots.
function isVisualFixture() {
  return import.meta.env.DEV
    && (new URLSearchParams(window.location.search).has("visualFixture")
      || window.location.hash.includes("visualFixture="));
}

export const useUiStore = defineStore("ui", () => {
  const appTheme = useAppThemeStore();
  const stored = readPreferences();
  const visualFixture = isVisualFixture();
  const storedTerminalAppearance = parseStoredTerminalAppearance(stored);
  const themePreference = ref<ThemePreference>(
    parseThemePreference(stored.themePreference, stored.theme),
  );
  const theme = ref<Theme>(resolveThemePreference(
    themePreference.value,
    window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false,
  ));
  const uiZoom = ref<UiZoom>(isUiZoom(stored.uiZoom) ? stored.uiZoom : 90);
  const appliedUiZoom = ref<UiZoom>(100);
  const uiZoomBusy = ref(false);
  const localePreference = ref<LocalePreference>(visualFixture
    ? "en"
    : (stored.locale === "en" || stored.locale === "zh-CN" ? stored.locale : "system"));
  const systemLanguages = ref<readonly string[]>(navigator.languages);
  const locale = computed(() => resolveLocale(localePreference.value, systemLanguages.value));
  const updateSystemLanguage = () => {
    systemLanguages.value = [...navigator.languages];
    applyDocumentPreferences();
  };
  window.addEventListener("languagechange", updateSystemLanguage);
  onScopeDispose(() => window.removeEventListener("languagechange", updateSystemLanguage));
  const terminalStartupBehavior = ref<TerminalStartupBehavior>(
    stored.terminalStartupBehavior === "welcome" ? "welcome" : "restoreHistory",
  );
  const newTerminalBehavior = ref<NewTerminalBehavior>(
    stored.newTerminalBehavior === "localTerminal" ? "localTerminal" : "welcome",
  );
  const singlePaneTabCloseBehavior = ref<SinglePaneTabCloseBehavior>(
    stored.singlePaneTabCloseBehavior === "closeDirectly" ? "closeDirectly" : "confirm",
  );
  const terminalThemeMode = ref<TerminalThemeMode>(storedTerminalAppearance.mode);
  const customTerminalPalette = ref(storedTerminalAppearance.customPalette);
  const customTerminalPaletteName = ref(storedTerminalAppearance.customName);
  const hasCustomTerminalPalette = ref(storedTerminalAppearance.hasCustomPalette);
  const terminalFontFamily = ref(
    normalizeTerminalFontFamily(stored.terminalFontFamily ?? DEFAULT_TERMINAL_FONT_FAMILY),
  );
  const terminalFontSize = ref(parseTerminalFontSize(stored.terminalFontSize));
  const terminalFontWeight = ref(parseTerminalFontWeight(stored.terminalFontWeight));
  const terminalBoldFontWeight = ref(parseTerminalFontWeight(
    stored.terminalBoldFontWeight,
    DEFAULT_TERMINAL_BOLD_FONT_WEIGHT,
  ));
  const terminalLineHeight = ref(parseTerminalLineHeight(stored.terminalLineHeight));
  const terminalLetterSpacing = ref(parseTerminalLetterSpacing(stored.terminalLetterSpacing));
  const terminalCursorStyle = ref(parseTerminalCursorStyle(stored.terminalCursorStyle));
  const terminalCursorBlink = ref(stored.terminalCursorBlink !== false);
  const resolvedTerminalPalette = computed(() =>
    resolveTerminalPalette(
      terminalThemeMode.value,
      theme.value,
      customTerminalPalette.value,
      appTheme.resolvedTheme.terminalPalette,
    ),
  );

  function serializedPreferences(overrides: Partial<StoredUiPreferences> = {}) {
    return {
      terminalAppearanceVersion: 5,
      theme: theme.value,
      themePreference: themePreference.value,
      uiZoom: uiZoom.value,
      locale: localePreference.value,
      terminalStartupBehavior: terminalStartupBehavior.value,
      newTerminalBehavior: newTerminalBehavior.value,
      singlePaneTabCloseBehavior: singlePaneTabCloseBehavior.value,
      terminalThemeMode: terminalThemeMode.value,
      terminalFontFamily: terminalFontFamily.value,
      terminalFontSize: terminalFontSize.value,
      terminalFontWeight: terminalFontWeight.value,
      terminalBoldFontWeight: terminalBoldFontWeight.value,
      terminalLineHeight: terminalLineHeight.value,
      terminalLetterSpacing: terminalLetterSpacing.value,
      terminalCursorStyle: terminalCursorStyle.value,
      terminalCursorBlink: terminalCursorBlink.value,
      customTerminalPalette: customTerminalPalette.value,
      customTerminalPaletteName: customTerminalPaletteName.value,
      ...overrides,
    };
  }

  function persistPreferences(overrides: Partial<StoredUiPreferences> = {}) {
    localStorage.setItem(
      UI_PREFERENCES_KEY,
      JSON.stringify(serializedPreferences(overrides)),
    );
  }

  async function commitApplication(next: ApplicationPreferences) {
    if (corePreferencesEnabled()) {
      if (!await saveApplicationPreferences("application", next, applicationPreferences())) return false;
      publishSecureWindowAppearance(next);
      return true;
    }
    try {
      persistPreferences({ ...next, theme: resolveThemePreference(next.themePreference, window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false) });
      return true;
    } catch { return false; }
  }

  async function commitAppearance(next: AppearancePreferences) {
    if (corePreferencesEnabled()) return saveApplicationPreferences("appearance", next, appearancePreferences());
    const uiAppearance: AppearancePreferences = { ...next };
    delete uiAppearance.appTheme;
    try { persistPreferences(uiAppearance); return true; }
    catch { return false; }
  }

  function hydrateCorePreferences(application: ApplicationPreferences, appearance: AppearancePreferences) {
    themePreference.value = application.themePreference;
    theme.value = resolveThemePreference(application.themePreference, window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false);
    localePreference.value = application.locale;
    uiZoom.value = application.uiZoom;
    terminalStartupBehavior.value = application.terminalStartupBehavior;
    newTerminalBehavior.value = application.newTerminalBehavior;
    singlePaneTabCloseBehavior.value = application.singlePaneTabCloseBehavior;
    terminalThemeMode.value = appearance.terminalThemeMode;
    terminalFontFamily.value = appearance.terminalFontFamily;
    terminalFontSize.value = appearance.terminalFontSize;
    terminalFontWeight.value = parseTerminalFontWeight(appearance.terminalFontWeight);
    terminalBoldFontWeight.value = parseTerminalFontWeight(appearance.terminalBoldFontWeight);
    terminalLineHeight.value = appearance.terminalLineHeight;
    terminalLetterSpacing.value = appearance.terminalLetterSpacing;
    terminalCursorStyle.value = appearance.terminalCursorStyle;
    terminalCursorBlink.value = appearance.terminalCursorBlink;
    customTerminalPalette.value = { ...appearance.customTerminalPalette };
    customTerminalPaletteName.value = appearance.customTerminalPaletteName;
    hasCustomTerminalPalette.value = appearance.terminalThemeMode === "custom" || appearance.customTerminalPaletteName.length > 0;
    appTheme.hydrateCorePreferences(appearance.appTheme ?? cloneDefaultAppThemeProfile());
    if (corePreferencesEnabled()) publishSecureWindowAppearance(application);
    syncSystemThemeListener();
    applyDocumentPreferences();
  }

  function applyDocumentPreferences() {
    document.documentElement.dataset.theme = theme.value;
    appTheme.applyTheme(theme.value);
    document.documentElement.lang = locale.value;
    i18n.global.locale.value = locale.value;
    for (const [key, cssVariable] of Object.entries(TERMINAL_PALETTE_CSS_VARIABLES)) {
      document.documentElement.style.setProperty(
        cssVariable,
        resolvedTerminalPalette.value[key as TerminalColorKey],
      );
    }
  }

  // Package updates can change a followed terminal palette without a UI preference change.
  watch(resolvedTerminalPalette, (palette) => {
    for (const [key, cssVariable] of Object.entries(TERMINAL_PALETTE_CSS_VARIABLES)) {
      document.documentElement.style.setProperty(cssVariable, palette[key as TerminalColorKey]);
    }
  });

  function applyPreferences() {
    applyDocumentPreferences();
    if (!corePreferencesEnabled()) persistPreferences();
  }

  let removeSystemThemeListener: (() => void) | undefined;

  function syncSystemThemeListener() {
    removeSystemThemeListener?.();
    removeSystemThemeListener = undefined;

    if (themePreference.value !== "system" || !window.matchMedia) return;

    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (themePreference.value !== "system") return;
      theme.value = resolveThemePreference("system", mediaQuery.matches);
      applyDocumentPreferences();
    };
    mediaQuery.addEventListener("change", onChange);
    removeSystemThemeListener = () => mediaQuery.removeEventListener("change", onChange);
  }

  syncSystemThemeListener();
  onScopeDispose(() => removeSystemThemeListener?.());

  async function setUiZoom(value: unknown, persist = true) {
    if (!isUiZoom(value) || uiZoomBusy.value) return false;
    uiZoomBusy.value = true;
    const previous = appliedUiZoom.value;
    try {
      await applyUiZoom(value);
      appliedUiZoom.value = value;
      if (persist && !await commitApplication({ ...applicationPreferences(), uiZoom: value })) throw new Error("preference save failed");
      uiZoom.value = value;
      return true;
    } catch (error) {
      // Restore the actual view when persistence fails; if native rollback fails, retain the actual zoom for layout avoidance.
      if (appliedUiZoom.value !== previous) {
        try {
          await applyUiZoom(previous);
          appliedUiZoom.value = previous;
        } catch { /* The Settings page reports failure and never presents an unknown native state as success. */ }
      }
      if (corePreferencesEnabled() && parseCoreApiError(error)?.code.startsWith("application_preferences.")) throw error;
      return false;
    } finally {
      uiZoomBusy.value = false;
    }
  }

  function applicationPreferences(): ApplicationPreferences {
    return { themePreference: themePreference.value, locale: localePreference.value, uiZoom: uiZoom.value,
      terminalStartupBehavior: terminalStartupBehavior.value, newTerminalBehavior: newTerminalBehavior.value,
      singlePaneTabCloseBehavior: singlePaneTabCloseBehavior.value };
  }

  function legacyApplicationPreferences(): ApplicationPreferences {
    return { ...applicationPreferences(), locale: stored.locale === "en" || stored.locale === "zh-CN" ? stored.locale : "system" };
  }

  function appearancePreferences(): AppearancePreferences {
    return { terminalThemeMode: terminalThemeMode.value, terminalFontFamily: terminalFontFamily.value,
      terminalFontSize: terminalFontSize.value, terminalFontWeight: terminalFontWeight.value,
      terminalBoldFontWeight: terminalBoldFontWeight.value, terminalLineHeight: terminalLineHeight.value,
      terminalLetterSpacing: terminalLetterSpacing.value, terminalCursorStyle: terminalCursorStyle.value,
      terminalCursorBlink: terminalCursorBlink.value, customTerminalPalette: { ...customTerminalPalette.value },
      customTerminalPaletteName: customTerminalPaletteName.value,
      appTheme: appTheme.appThemePreferences() };
  }

  async function replaceApplicationPreferences(next: unknown, expected: ApplicationPreferences) {
    if (!validateApplicationPreferences(next) || uiZoomBusy.value
      || JSON.stringify(applicationPreferences()) !== JSON.stringify(expected)) return false;
    uiZoomBusy.value = true;
    const previousZoom = appliedUiZoom.value;
    try {
      if (previousZoom !== next.uiZoom) {
        await applyUiZoom(next.uiZoom);
        appliedUiZoom.value = next.uiZoom;
      }
      if (JSON.stringify(applicationPreferences()) !== JSON.stringify(expected)) throw new Error("preference changed");
      const nextTheme = resolveThemePreference(next.themePreference, window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false);
      if (!await commitApplication(next)) throw new Error("preference save failed");
      themePreference.value = next.themePreference;
      theme.value = nextTheme;
      localePreference.value = next.locale;
      uiZoom.value = next.uiZoom;
      terminalStartupBehavior.value = next.terminalStartupBehavior;
      newTerminalBehavior.value = next.newTerminalBehavior;
      singlePaneTabCloseBehavior.value = next.singlePaneTabCloseBehavior;
      syncSystemThemeListener();
      applyDocumentPreferences();
      return true;
    } catch (error) {
      if (appliedUiZoom.value !== previousZoom) {
        try { await applyUiZoom(previousZoom); appliedUiZoom.value = previousZoom; }
        catch { /* Retain the actual zoom projection and let the caller report failure. */ }
      }
      if (corePreferencesEnabled() && parseCoreApiError(error)?.code.startsWith("application_preferences.")) throw error;
      return false;
    } finally { uiZoomBusy.value = false; }
  }

  async function replaceAppearancePreferences(next: unknown, expected: AppearancePreferences) {
    if (!validateAppearancePreferences(next)
      || JSON.stringify(appearancePreferences()) !== JSON.stringify(expected)) return false;
    const expectedThemeProfile = expected.appTheme ?? appTheme.appThemePreferences();
    const nextThemeProfile = next.appTheme ?? expectedThemeProfile;
    if (corePreferencesEnabled()) {
      if (!await commitAppearance({ ...next, appTheme: nextThemeProfile })) return false;
      appTheme.hydrateCorePreferences(nextThemeProfile);
    } else {
      if (!await appTheme.replaceAppThemePreferences(nextThemeProfile, expectedThemeProfile)) return false;
      if (!await commitAppearance(next)) {
        await appTheme.replaceAppThemePreferences(expectedThemeProfile, nextThemeProfile);
        return false;
      }
    }
    terminalThemeMode.value = next.terminalThemeMode;
    terminalFontFamily.value = next.terminalFontFamily;
    terminalFontSize.value = next.terminalFontSize;
    terminalFontWeight.value = parseTerminalFontWeight(next.terminalFontWeight);
    terminalBoldFontWeight.value = parseTerminalFontWeight(next.terminalBoldFontWeight);
    terminalLineHeight.value = next.terminalLineHeight;
    terminalLetterSpacing.value = next.terminalLetterSpacing;
    terminalCursorStyle.value = next.terminalCursorStyle;
    terminalCursorBlink.value = next.terminalCursorBlink;
    customTerminalPalette.value = { ...next.customTerminalPalette };
    customTerminalPaletteName.value = next.customTerminalPaletteName;
    hasCustomTerminalPalette.value = next.terminalThemeMode === "custom" || next.customTerminalPaletteName.length > 0;
    applyDocumentPreferences();
    return true;
  }

  async function setLocale(value: LocalePreference) {
    if (!await commitApplication({ ...applicationPreferences(), locale: value })) return false;
    localePreference.value = value;
    applyDocumentPreferences();
    return true;
  }

  async function setTerminalStartupBehavior(value: TerminalStartupBehavior) {
    if (!await commitApplication({ ...applicationPreferences(), terminalStartupBehavior: value })) return false;
    terminalStartupBehavior.value = value;
    return true;
  }

  async function setNewTerminalBehavior(value: NewTerminalBehavior) {
    if (!await commitApplication({ ...applicationPreferences(), newTerminalBehavior: value })) return false;
    newTerminalBehavior.value = value;
    return true;
  }

  async function setSinglePaneTabCloseBehavior(value: SinglePaneTabCloseBehavior) {
    if (!await commitApplication({ ...applicationPreferences(), singlePaneTabCloseBehavior: value })) return false;
    singlePaneTabCloseBehavior.value = value;
    return true;
  }

  async function toggleTheme() {
    return setThemePreference(theme.value === "light" ? "dark" : "light");
  }

  function setTheme(value: Theme) {
    return setThemePreference(value);
  }

  async function setThemePreference(value: ThemePreference) {
    const nextTheme = resolveThemePreference(
      value,
      window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false,
    );

    if (!await commitApplication({ ...applicationPreferences(), themePreference: value })) return false;

    themePreference.value = value;
    theme.value = nextTheme;
    syncSystemThemeListener();
    applyDocumentPreferences();
    return true;
  }

  async function updateAppearance(patch: Partial<AppearancePreferences>) {
    const next = { ...appearancePreferences(), ...patch };
    if (!validateAppearancePreferences(next) || !await commitAppearance(next)) return false;
    hydrateCorePreferences(applicationPreferences(), next);
    return true;
  }

  function setTerminalThemeMode(value: TerminalThemeMode) { return updateAppearance({ terminalThemeMode: value }); }
  function setTerminalFontFamily(value: string) { return updateAppearance({ terminalFontFamily: normalizeTerminalFontFamily(value) }); }
  function setTerminalFontSize(value: string | number) { return updateAppearance({ terminalFontSize: parseTerminalFontSize(value) }); }
  function setTerminalFontWeight(value: string | number) { return updateAppearance({ terminalFontWeight: parseTerminalFontWeight(value) }); }
  function setTerminalBoldFontWeight(value: string | number) {
    return updateAppearance({ terminalBoldFontWeight: parseTerminalFontWeight(value, DEFAULT_TERMINAL_BOLD_FONT_WEIGHT) });
  }
  function setTerminalLineHeight(value: string | number) { return updateAppearance({ terminalLineHeight: parseTerminalLineHeight(value) }); }
  function setTerminalLetterSpacing(value: string | number) { return updateAppearance({ terminalLetterSpacing: parseTerminalLetterSpacing(value) }); }
  function setTerminalCursorStyle(value: TerminalCursorStyle) { return updateAppearance({ terminalCursorStyle: parseTerminalCursorStyle(value) }); }
  function setTerminalCursorBlink(value: boolean) { return updateAppearance({ terminalCursorBlink: value }); }
  function setCustomTerminalColor(key: TerminalColorKey, value: string) {
    if (!isHexColor(value)) return Promise.resolve(false);
    return updateAppearance({ customTerminalPalette: { ...customTerminalPalette.value, [key]: value.toLowerCase() } });
  }
  function resetCustomTerminalPalette() {
    return updateAppearance({ customTerminalPalette: cloneTerminalPalette(theme.value === "light" ? LIGHT_TERMINAL_PALETTE : DARK_TERMINAL_PALETTE) });
  }

  async function saveCustomTerminalPalette(name: string, palette: TerminalPalette) {
    const customName = normalizeCustomTerminalPaletteName(name);
    const parsedPalette = parseTerminalPalette(palette);
    if (!customName || !parsedPalette) return false;

    return updateAppearance({ terminalThemeMode: "custom", customTerminalPalette: parsedPalette, customTerminalPaletteName: customName });
  }

  return {
    theme,
    themePreference,
    uiZoom,
    appliedUiZoom,
    uiZoomBusy,
    setUiZoom,
    applicationPreferences,
    legacyApplicationPreferences,
    appearancePreferences,
    hydrateCorePreferences,
    replaceApplicationPreferences,
    replaceAppearancePreferences,
    locale,
    localePreference,
    terminalStartupBehavior,
    newTerminalBehavior,
    singlePaneTabCloseBehavior,
    terminalThemeMode,
    terminalFontFamily,
    terminalFontSize,
    terminalFontWeight,
    terminalBoldFontWeight,
    terminalLineHeight,
    terminalLetterSpacing,
    terminalCursorStyle,
    terminalCursorBlink,
    customTerminalPalette,
    customTerminalPaletteName,
    hasCustomTerminalPalette,
    resolvedTerminalPalette,
    applyPreferences,
    setLocale,
    setTerminalStartupBehavior,
    setNewTerminalBehavior,
    setSinglePaneTabCloseBehavior,
    setTheme,
    setThemePreference,
    setTerminalThemeMode,
    setTerminalFontFamily,
    setTerminalFontSize,
    setTerminalFontWeight,
    setTerminalBoldFontWeight,
    setTerminalLineHeight,
    setTerminalLetterSpacing,
    setTerminalCursorStyle,
    setTerminalCursorBlink,
    setCustomTerminalColor,
    resetCustomTerminalPalette,
    saveCustomTerminalPalette,
    toggleTheme,
  };
});
