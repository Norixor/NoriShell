import { defineStore } from "pinia";
import { computed, onScopeDispose, ref, watch } from "vue";

import { useAppThemeStore } from "./appTheme";
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
  const uiZoom = ref<UiZoom>(isUiZoom(stored.uiZoom) ? stored.uiZoom : 100);
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
    persistPreferences();
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
      if (persist) persistPreferences({ uiZoom: value });
      uiZoom.value = value;
      return true;
    } catch {
      // Restore the actual view when persistence fails; if native rollback fails, retain the actual zoom for layout avoidance.
      if (appliedUiZoom.value !== previous) {
        try {
          await applyUiZoom(previous);
          appliedUiZoom.value = previous;
        } catch { /* The Settings page reports failure and never presents an unknown native state as success. */ }
      }
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
      persistPreferences({ ...next, theme: nextTheme });
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
    } catch {
      if (appliedUiZoom.value !== previousZoom) {
        try { await applyUiZoom(previousZoom); appliedUiZoom.value = previousZoom; }
        catch { /* Retain the actual zoom projection and let the caller report failure. */ }
      }
      return false;
    } finally { uiZoomBusy.value = false; }
  }

  function replaceAppearancePreferences(next: unknown, expected: AppearancePreferences) {
    if (!validateAppearancePreferences(next)
      || JSON.stringify(appearancePreferences()) !== JSON.stringify(expected)) return false;
    const expectedThemeProfile = expected.appTheme ?? appTheme.appThemePreferences();
    const nextThemeProfile = next.appTheme ?? expectedThemeProfile;
    if (!appTheme.replaceAppThemePreferences(nextThemeProfile, expectedThemeProfile)) return false;
    const uiAppearance: AppearancePreferences = { ...next };
    delete uiAppearance.appTheme;
    try { persistPreferences(uiAppearance); } catch {
      // The transfer group must not report success after only the separate profile was saved.
      appTheme.replaceAppThemePreferences(expectedThemeProfile, nextThemeProfile);
      return false;
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

  function setLocale(value: LocalePreference) {
    persistPreferences({ locale: value });
    localePreference.value = value;
    applyDocumentPreferences();
  }

  function setTerminalStartupBehavior(value: TerminalStartupBehavior) {
    terminalStartupBehavior.value = value;
    applyPreferences();
  }

  function setNewTerminalBehavior(value: NewTerminalBehavior) {
    newTerminalBehavior.value = value;
    applyPreferences();
  }

  function setSinglePaneTabCloseBehavior(value: SinglePaneTabCloseBehavior) {
    singlePaneTabCloseBehavior.value = value;
    applyPreferences();
  }

  function toggleTheme() {
    setThemePreference(theme.value === "light" ? "dark" : "light");
  }

  function setTheme(value: Theme) {
    return setThemePreference(value);
  }

  function setThemePreference(value: ThemePreference) {
    const nextTheme = resolveThemePreference(
      value,
      window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false,
    );

    try {
      persistPreferences({ theme: nextTheme, themePreference: value });
    } catch {
      return false;
    }

    themePreference.value = value;
    theme.value = nextTheme;
    syncSystemThemeListener();
    applyDocumentPreferences();
    return true;
  }

  function setTerminalThemeMode(value: TerminalThemeMode) {
    terminalThemeMode.value = value;
    applyPreferences();
  }

  function setTerminalFontFamily(value: string) {
    terminalFontFamily.value = normalizeTerminalFontFamily(value);
    applyPreferences();
  }

  function setTerminalFontSize(value: string | number) {
    terminalFontSize.value = parseTerminalFontSize(value);
    applyPreferences();
  }

  function setTerminalFontWeight(value: string | number) {
    terminalFontWeight.value = parseTerminalFontWeight(value);
    applyPreferences();
  }

  function setTerminalBoldFontWeight(value: string | number) {
    terminalBoldFontWeight.value = parseTerminalFontWeight(
      value,
      DEFAULT_TERMINAL_BOLD_FONT_WEIGHT,
    );
    applyPreferences();
  }

  function setTerminalLineHeight(value: string | number) {
    terminalLineHeight.value = parseTerminalLineHeight(value);
    applyPreferences();
  }

  function setTerminalLetterSpacing(value: string | number) {
    terminalLetterSpacing.value = parseTerminalLetterSpacing(value);
    applyPreferences();
  }

  function setTerminalCursorStyle(value: TerminalCursorStyle) {
    terminalCursorStyle.value = parseTerminalCursorStyle(value);
    applyPreferences();
  }

  function setTerminalCursorBlink(value: boolean) {
    terminalCursorBlink.value = value;
    applyPreferences();
  }

  function setCustomTerminalColor(key: TerminalColorKey, value: string) {
    if (!isHexColor(value)) return;
    customTerminalPalette.value = {
      ...customTerminalPalette.value,
      [key]: value.toLowerCase(),
    };
    applyPreferences();
  }

  function resetCustomTerminalPalette() {
    customTerminalPalette.value = cloneTerminalPalette(
      theme.value === "light" ? LIGHT_TERMINAL_PALETTE : DARK_TERMINAL_PALETTE,
    );
    applyPreferences();
  }

  function saveCustomTerminalPalette(name: string, palette: TerminalPalette) {
    const customName = normalizeCustomTerminalPaletteName(name);
    const parsedPalette = parseTerminalPalette(palette);
    if (!customName || !parsedPalette) return false;

    try {
      persistPreferences({
        terminalThemeMode: "custom",
        customTerminalPalette: parsedPalette,
        customTerminalPaletteName: customName,
      });
    } catch {
      return false;
    }

    customTerminalPaletteName.value = customName;
    customTerminalPalette.value = parsedPalette;
    hasCustomTerminalPalette.value = true;
    terminalThemeMode.value = "custom";
    applyDocumentPreferences();
    return true;
  }

  return {
    theme,
    themePreference,
    uiZoom,
    appliedUiZoom,
    uiZoomBusy,
    setUiZoom,
    applicationPreferences,
    appearancePreferences,
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
