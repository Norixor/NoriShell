import type { LocalePreference } from "./locales";
import { isUiZoom, type UiZoom } from "./ui-zoom";
import type { ThemePreference } from "./ui-preferences";
import {
  validateAppThemeProfile,
  type AppThemeProfile,
} from "./app-theme";
import {
  cloneTerminalPalette, DARK_TERMINAL_PALETTE, DEFAULT_TERMINAL_BOLD_FONT_WEIGHT,
  DEFAULT_TERMINAL_FONT_FAMILY, DEFAULT_TERMINAL_FONT_SIZE, DEFAULT_TERMINAL_FONT_WEIGHT,
  DEFAULT_TERMINAL_LETTER_SPACING, DEFAULT_TERMINAL_LINE_HEIGHT, isTerminalPresetId,
  normalizeCustomTerminalPaletteName, normalizeTerminalFontFamily, parseTerminalFontSize,
  parseTerminalFontWeight, parseTerminalLetterSpacing, parseTerminalLineHeight,
  parseTerminalPalette, TERMINAL_COLOR_KEYS,
  type TerminalCursorStyle, type TerminalPalette, type TerminalThemeMode,
} from "./terminal-theme";

export interface ApplicationPreferences {
  themePreference: ThemePreference;
  locale: LocalePreference;
  uiZoom: UiZoom;
  terminalStartupBehavior: "welcome" | "restoreHistory";
  newTerminalBehavior: "welcome" | "localTerminal";
  singlePaneTabCloseBehavior: "confirm" | "closeDirectly";
}
export interface AppearancePreferences {
  terminalThemeMode: TerminalThemeMode;
  terminalFontFamily: string;
  terminalFontSize: number;
  terminalFontWeight: number;
  terminalBoldFontWeight: number;
  terminalLineHeight: number;
  terminalLetterSpacing: number;
  terminalCursorStyle: TerminalCursorStyle;
  terminalCursorBlink: boolean;
  customTerminalPalette: TerminalPalette;
  customTerminalPaletteName: string;
  // v1 exports include this separate namespace profile. Older appearance files omit it.
  appTheme?: AppThemeProfile;
}

export const DEFAULT_APPLICATION_PREFERENCES: ApplicationPreferences = {
  themePreference: "light", locale: "system", uiZoom: 100,
  terminalStartupBehavior: "restoreHistory", newTerminalBehavior: "welcome", singlePaneTabCloseBehavior: "confirm",
};
export function defaultAppearancePreferences(): AppearancePreferences {
  return {
    terminalThemeMode: "follow-app", terminalFontFamily: DEFAULT_TERMINAL_FONT_FAMILY,
    terminalFontSize: DEFAULT_TERMINAL_FONT_SIZE, terminalFontWeight: DEFAULT_TERMINAL_FONT_WEIGHT,
    terminalBoldFontWeight: DEFAULT_TERMINAL_BOLD_FONT_WEIGHT, terminalLineHeight: DEFAULT_TERMINAL_LINE_HEIGHT,
    terminalLetterSpacing: DEFAULT_TERMINAL_LETTER_SPACING, terminalCursorStyle: "block", terminalCursorBlink: true,
    customTerminalPalette: cloneTerminalPalette(DARK_TERMINAL_PALETTE), customTerminalPaletteName: "",
  };
}

export function exactPreferenceKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === keys.length && Object.keys(value).every((key) => keys.includes(key));
}
export function validateApplicationPreferences(value: unknown): value is ApplicationPreferences {
  if (!exactPreferenceKeys(value, Object.keys(DEFAULT_APPLICATION_PREFERENCES))) return false;
  return ["light", "dark", "system"].includes(value.themePreference as string)
    && ["system", "zh-CN", "en"].includes(value.locale as string) && isUiZoom(value.uiZoom)
    && ["welcome", "restoreHistory"].includes(value.terminalStartupBehavior as string)
    && ["welcome", "localTerminal"].includes(value.newTerminalBehavior as string)
    && ["confirm", "closeDirectly"].includes(value.singlePaneTabCloseBehavior as string);
}
export function validateAppearancePreferences(value: unknown): value is AppearancePreferences {
  const baseKeys = Object.keys(defaultAppearancePreferences());
  if (!exactPreferenceKeys(value, baseKeys) && !exactPreferenceKeys(value, [...baseKeys, "appTheme"])) return false;
  return (value.terminalThemeMode === "follow-app" || value.terminalThemeMode === "custom" || isTerminalPresetId(value.terminalThemeMode))
    && typeof value.terminalFontFamily === "string" && value.terminalFontFamily === normalizeTerminalFontFamily(value.terminalFontFamily)
    && typeof value.terminalFontSize === "number" && value.terminalFontSize === parseTerminalFontSize(value.terminalFontSize)
    && typeof value.terminalFontWeight === "number" && value.terminalFontWeight === parseTerminalFontWeight(value.terminalFontWeight)
    && typeof value.terminalBoldFontWeight === "number" && value.terminalBoldFontWeight === parseTerminalFontWeight(value.terminalBoldFontWeight)
    && typeof value.terminalLineHeight === "number" && value.terminalLineHeight === parseTerminalLineHeight(value.terminalLineHeight)
    && typeof value.terminalLetterSpacing === "number" && value.terminalLetterSpacing === parseTerminalLetterSpacing(value.terminalLetterSpacing)
    && ["block", "bar", "underline"].includes(value.terminalCursorStyle as string)
    && typeof value.terminalCursorBlink === "boolean"
    && exactPreferenceKeys(value.customTerminalPalette, TERMINAL_COLOR_KEYS) && parseTerminalPalette(value.customTerminalPalette) !== null
    && typeof value.customTerminalPaletteName === "string" && value.customTerminalPaletteName === normalizeCustomTerminalPaletteName(value.customTerminalPaletteName)
    && (value.appTheme === undefined || validateAppThemeProfile(value.appTheme));
}
