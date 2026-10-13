import type { LocalePreference } from "./locales";
import { isUiZoom, type UiZoom } from "./ui-zoom";
import type { ThemePreference } from "./ui-preferences";
import {
  validateAppThemeProfile,
  type AppThemeProfile,
} from "./app-theme";
import {
  cloneTerminalPalette, DARK_TERMINAL_PALETTE, DEFAULT_TERMINAL_BOLD_FONT_WEIGHT,
  DEFAULT_TERMINAL_FONT_FAMILY, DEFAULT_TERMINAL_FONT_WEIGHT, defaultTerminalFontSize,
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
  terminalStartupBehavior: TerminalStartupBehavior;
  newTerminalBehavior: "welcome" | "terminalWelcome" | "sftpWelcome" | "localTerminal";
  singlePaneTabCloseBehavior: "confirm" | "closeDirectly";
  /** Optional on the wire: data written before this field existed omits it and means "default". */
  localTerminalShell?: string;
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

/** "overview" was added later; it is only the default for never-set preferences, never a migration target. */
export const TERMINAL_STARTUP_BEHAVIORS = ["overview", "welcome", "restoreHistory"] as const;
export type TerminalStartupBehavior = typeof TERMINAL_STARTUP_BEHAVIORS[number];
export const DEFAULT_TERMINAL_STARTUP_BEHAVIOR: TerminalStartupBehavior = "overview";
export function isTerminalStartupBehavior(value: unknown): value is TerminalStartupBehavior {
  return (TERMINAL_STARTUP_BEHAVIORS as readonly unknown[]).includes(value);
}

export const DEFAULT_APPLICATION_PREFERENCES: ApplicationPreferences = {
  themePreference: "light", locale: "system", uiZoom: 100,
  terminalStartupBehavior: DEFAULT_TERMINAL_STARTUP_BEHAVIOR, newTerminalBehavior: "welcome", singlePaneTabCloseBehavior: "confirm",
  localTerminalShell: "default",
};

export const DEFAULT_LOCAL_TERMINAL_SHELL = "default";
const FIXED_LOCAL_TERMINAL_SHELL_IDS = ["default", "cmd", "windowsPowerShell", "powershell7", "gitBash"];
const LOCAL_TERMINAL_SHELL_PATH_PREFIX = "path:";
export const LOCAL_TERMINAL_SHELL_PATH_MAX_UTF16 = 1024;

/** Why a custom shell path is rejected; mirrors the Core validation (UTF-16 length 1..=1024, no control chars, no edge whitespace). */
export type LocalTerminalShellPathIssue = "empty" | "tooLong" | "controlCharacter";
export function localTerminalShellPathIssue(path: string): LocalTerminalShellPathIssue | null {
  const trimmed = path.trim();
  if (trimmed.length === 0) return "empty";
  if (trimmed.length > LOCAL_TERMINAL_SHELL_PATH_MAX_UTF16) return "tooLong";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/u.test(trimmed)) return "controlCharacter";
  return null;
}
export function isLocalTerminalShell(value: unknown): value is string {
  if (typeof value !== "string") return false;
  if (FIXED_LOCAL_TERMINAL_SHELL_IDS.includes(value)) return true;
  if (!value.startsWith(LOCAL_TERMINAL_SHELL_PATH_PREFIX)) return false;
  const path = value.slice(LOCAL_TERMINAL_SHELL_PATH_PREFIX.length);
  return path === path.trim() && localTerminalShellPathIssue(path) === null;
}
export function localTerminalShellPath(value: string): string | null {
  return value.startsWith(LOCAL_TERMINAL_SHELL_PATH_PREFIX) ? value.slice(LOCAL_TERMINAL_SHELL_PATH_PREFIX.length) : null;
}
export function customLocalTerminalShell(path: string): string {
  return `${LOCAL_TERMINAL_SHELL_PATH_PREFIX}${path.trim()}`;
}
/** Fills the optional shell field so callers can compare and persist one canonical shape. */
export function normalizeApplicationPreferences<T extends { localTerminalShell?: unknown }>(value: T): T & { localTerminalShell: string } {
  return { ...value, localTerminalShell: typeof value.localTerminalShell === "string" ? value.localTerminalShell : DEFAULT_LOCAL_TERMINAL_SHELL };
}
export function defaultAppearancePreferences(): AppearancePreferences {
  return {
    terminalThemeMode: "follow-app", terminalFontFamily: DEFAULT_TERMINAL_FONT_FAMILY,
    terminalFontSize: defaultTerminalFontSize(), terminalFontWeight: DEFAULT_TERMINAL_FONT_WEIGHT,
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
  const keys = Object.keys(DEFAULT_APPLICATION_PREFERENCES);
  // Files and Core records written before localTerminalShell existed omit it and stay valid.
  if (!exactPreferenceKeys(value, keys) && !exactPreferenceKeys(value, keys.filter((key) => key !== "localTerminalShell"))) return false;
  if (value.localTerminalShell !== undefined && !isLocalTerminalShell(value.localTerminalShell)) return false;
  return ["light", "dark", "system"].includes(value.themePreference as string)
    && ["system", "zh-CN", "en"].includes(value.locale as string) && isUiZoom(value.uiZoom)
    && isTerminalStartupBehavior(value.terminalStartupBehavior)
    && ["welcome", "terminalWelcome", "sftpWelcome", "localTerminal"].includes(value.newTerminalBehavior as string)
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
