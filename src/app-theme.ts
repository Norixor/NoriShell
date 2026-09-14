import {
  DARK_TERMINAL_PALETTE,
  LIGHT_TERMINAL_PALETTE,
  TERMINAL_COLOR_KEYS,
  isHexColor,
  type TerminalPalette,
} from "./terminal-theme";

export const APP_THEME_SCHEMA_VERSION = 1;
export const APP_THEME_PROFILE_SCHEMA_VERSION = 1;
export const APP_THEME_MAX_BYTES = 32 * 1024;
export const APP_THEME_MAX_OVERRIDES = 32;

export const APP_THEME_COLOR_KEYS = [
  "bgCanvas", "bgSurface", "bgSubtle", "bgHover", "border", "borderStrong",
  "textPrimary", "textSecondary", "textTertiary", "accent", "accentHover",
  "onAccent", "accentSoft", "success", "successSoft", "warning", "warningSoft",
  "danger", "onDanger", "dangerSoft", "focusRing", "terminalPaneActiveBorder",
  "brandMarkPrimary", "brandMarkSecondary", "selection", "selectionText",
] as const;

export type AppThemeColorKey = (typeof APP_THEME_COLOR_KEYS)[number];
export type ThemeAppearance = "light" | "dark";
export type ThemeFontFamily = "system" | "sans" | "mono";
export type ThemeDensity = "compact" | "standard" | "comfortable";
export type ThemeShadow = "none" | "soft" | "standard";
export type ThemeChoiceKey = string;

export interface ThemeDefinition {
  schemaVersion: 1;
  id: string;
  name: { zhCN: string; en: string };
  appearance: ThemeAppearance;
  colors: Record<AppThemeColorKey, string>;
  fontFamily: ThemeFontFamily;
  fontSize: number;
  radius: number;
  borderWidth: 1 | 2;
  density: ThemeDensity;
  shadow: ThemeShadow;
  terminalPalette?: TerminalPalette;
}

export interface ThemeOverride {
  colors?: Partial<Record<AppThemeColorKey, string>>;
  fontFamily?: ThemeFontFamily;
  fontSize?: number;
  radius?: number;
  borderWidth?: 1 | 2;
  density?: ThemeDensity;
  shadow?: ThemeShadow;
}

export interface AppThemeProfile {
  schemaVersion: 1;
  lightThemeId: ThemeChoiceKey;
  darkThemeId: ThemeChoiceKey;
  overrides: Record<ThemeChoiceKey, ThemeOverride>;
}

export const APP_THEME_COLOR_CSS_VARIABLES: Record<AppThemeColorKey, string> = {
  bgCanvas: "--nvx-color-bg-canvas",
  bgSurface: "--nvx-color-bg-surface",
  bgSubtle: "--nvx-color-bg-subtle",
  bgHover: "--nvx-color-bg-hover",
  border: "--nvx-color-border",
  borderStrong: "--nvx-color-border-strong",
  textPrimary: "--nvx-color-text-primary",
  textSecondary: "--nvx-color-text-secondary",
  textTertiary: "--nvx-color-text-tertiary",
  accent: "--nvx-color-accent",
  accentHover: "--nvx-color-accent-hover",
  onAccent: "--nvx-color-on-accent",
  accentSoft: "--nvx-color-accent-soft",
  success: "--nvx-color-success",
  successSoft: "--nvx-color-success-soft",
  warning: "--nvx-color-warning",
  warningSoft: "--nvx-color-warning-soft",
  danger: "--nvx-color-danger",
  onDanger: "--nvx-color-on-danger",
  dangerSoft: "--nvx-color-danger-soft",
  focusRing: "--nvx-color-focus-ring",
  terminalPaneActiveBorder: "--nvx-color-terminal-pane-active-border",
  brandMarkPrimary: "--nvx-brand-mark-primary",
  brandMarkSecondary: "--nvx-brand-mark-secondary",
  selection: "--nvx-color-selection",
  selectionText: "--nvx-color-selection-text",
};

const lightColors: Record<AppThemeColorKey, string> = {
  bgCanvas: "#f7f8fa", bgSurface: "#ffffff", bgSubtle: "#f1f3f6", bgHover: "#eaeef4",
  border: "#dce1e8", borderStrong: "#c4cbd6", textPrimary: "#171a21", textSecondary: "#525866",
  textTertiary: "#747e8e", accent: "#1f5fd2", accentHover: "#184faf", onAccent: "#ffffff",
  accentSoft: "#edf3ff", success: "#25834b", successSoft: "#edf8f1", warning: "#a96416",
  warningSoft: "#fff6e8", danger: "#be3d47", onDanger: "#ffffff", dangerSoft: "#fff0f1",
  focusRing: "#2f6fed", terminalPaneActiveBorder: "#c7ccd5", brandMarkPrimary: "#1f5fd2",
  brandMarkSecondary: "#6e9bff", selection: "#c9dafe", selectionText: "#171a21",
};
const darkColors: Record<AppThemeColorKey, string> = {
  bgCanvas: "#101217", bgSurface: "#171a20", bgSubtle: "#1e222a", bgHover: "#252b35",
  border: "#2c323d", borderStrong: "#414958", textPrimary: "#f2f4f7", textSecondary: "#b6bfcc",
  textTertiary: "#8c97a8", accent: "#6e9bff", accentHover: "#8baeff", onAccent: "#101217",
  accentSoft: "#1b2b4a", success: "#55b978", successSoft: "#193425", warning: "#e3a04e",
  warningSoft: "#3a2a16", danger: "#f07880", onDanger: "#101217", dangerSoft: "#3a1f23",
  focusRing: "#8aaeff", terminalPaneActiveBorder: "#303744", brandMarkPrimary: "#1f5fd2",
  brandMarkSecondary: "#6e9bff", selection: "#33415f", selectionText: "#f2f4f7",
};

export const BUILTIN_LIGHT_THEME_ID = "builtin:light";
export const BUILTIN_DARK_THEME_ID = "builtin:dark";

export const BUILTIN_APP_THEMES: Readonly<Record<ThemeAppearance, ThemeDefinition>> = {
  light: {
    schemaVersion: 1, id: "norishell-light", name: { zhCN: "NoriShell 浅色", en: "NoriShell Light" },
    appearance: "light", colors: lightColors, fontFamily: "sans", fontSize: 14, radius: 6,
    borderWidth: 1, density: "standard", shadow: "standard", terminalPalette: LIGHT_TERMINAL_PALETTE,
  },
  dark: {
    schemaVersion: 1, id: "norishell-dark", name: { zhCN: "NoriShell 深色", en: "NoriShell Dark" },
    appearance: "dark", colors: darkColors, fontFamily: "sans", fontSize: 14, radius: 6,
    borderWidth: 1, density: "standard", shadow: "standard", terminalPalette: DARK_TERMINAL_PALETTE,
  },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value: unknown, keys: readonly string[]) {
  return isRecord(value) && Object.keys(value).length === keys.length
    && Object.keys(value).every((key) => keys.includes(key));
}

function byteLength(value: unknown) {
  try {
    const serialized = JSON.stringify(value);
    return typeof serialized === "string" ? new TextEncoder().encode(serialized).byteLength : Number.POSITIVE_INFINITY;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

function boundedLabel(value: unknown) {
  return typeof value === "string" && value.trim().length > 0 && value.length <= 80
    && ![...value].some((character) => {
      const point = character.codePointAt(0) ?? 0;
      return point < 32 || point === 127;
    });
}

function channel(value: string) { return Number.parseInt(value.slice(1), 16) / 255; }
function linear(channelValue: number) {
  return channelValue <= 0.04045 ? channelValue / 12.92 : ((channelValue + 0.055) / 1.055) ** 2.4;
}
function luminance(value: string) {
  const red = linear(channel(value.slice(0, 3)));
  const green = linear(channel(`#${value.slice(3, 5)}`));
  const blue = linear(channel(`#${value.slice(5, 7)}`));
  return red * 0.2126 + green * 0.7152 + blue * 0.0722;
}
export function contrastRatio(first: string, second: string) {
  const left = luminance(first);
  const right = luminance(second);
  return (Math.max(left, right) + 0.05) / (Math.min(left, right) + 0.05);
}

function validTerminalPalette(value: unknown): value is TerminalPalette {
  return exactKeys(value, TERMINAL_COLOR_KEYS)
    && TERMINAL_COLOR_KEYS.every((key) => isHexColor((value as Record<string, unknown>)[key]));
}

export function validateThemeDefinition(value: unknown): value is ThemeDefinition {
  if (!isRecord(value) || byteLength(value) > APP_THEME_MAX_BYTES) return false;
  const candidate = value as Record<string, unknown>;
  const keys = ["schemaVersion", "id", "name", "appearance", "colors", "fontFamily", "fontSize", "radius", "borderWidth", "density", "shadow", "terminalPalette"];
  if (!Object.keys(candidate).every((key) => keys.includes(key)) || Object.keys(candidate).length < keys.length - 1) return false;
  if (candidate.schemaVersion !== APP_THEME_SCHEMA_VERSION || typeof candidate.id !== "string" || candidate.id.length === 0
    || candidate.id.length > 64 || !/^[a-z0-9]+(?:[._-][a-z0-9]+)*$/u.test(candidate.id)) return false;
  const name = candidate.name as Record<string, unknown>;
  if (!exactKeys(name, ["zhCN", "en"]) || !boundedLabel(name.zhCN) || !boundedLabel(name.en)) return false;
  if (candidate.appearance !== "light" && candidate.appearance !== "dark") return false;
  const colors = candidate.colors as Record<AppThemeColorKey, unknown>;
  if (!exactKeys(colors, APP_THEME_COLOR_KEYS) || !APP_THEME_COLOR_KEYS.every((key) => isHexColor(colors[key]))) return false;
  if (!["system", "sans", "mono"].includes(candidate.fontFamily as string)
    || !Number.isInteger(candidate.fontSize) || (candidate.fontSize as number) < 12 || (candidate.fontSize as number) > 18
    || !Number.isInteger(candidate.radius) || (candidate.radius as number) < 0 || (candidate.radius as number) > 12
    || (candidate.borderWidth !== 1 && candidate.borderWidth !== 2)
    || !["compact", "standard", "comfortable"].includes(candidate.density as string)
    || !["none", "soft", "standard"].includes(candidate.shadow as string)
    || (candidate.terminalPalette !== undefined && !validTerminalPalette(candidate.terminalPalette))) return false;
  const checkedColors = colors as Record<AppThemeColorKey, string>;
  for (const background of [checkedColors.bgCanvas, checkedColors.bgSurface, checkedColors.bgSubtle]) {
    if (contrastRatio(checkedColors.textPrimary, background) < 4.5 || contrastRatio(checkedColors.textSecondary, background) < 4.5) return false;
  }
  return contrastRatio(checkedColors.onAccent, checkedColors.accent) >= 4.5
    && contrastRatio(checkedColors.onAccent, checkedColors.accentHover) >= 4.5
    && contrastRatio(checkedColors.onDanger, checkedColors.danger) >= 4.5
    && contrastRatio(checkedColors.focusRing, checkedColors.bgCanvas) >= 3
    && contrastRatio(checkedColors.selectionText, checkedColors.selection) >= 4.5
    && [checkedColors.accentSoft, checkedColors.successSoft, checkedColors.warningSoft, checkedColors.dangerSoft]
      .every((background) => contrastRatio(checkedColors.textPrimary, background) >= 4.5)
    && ([[checkedColors.success, checkedColors.successSoft], [checkedColors.warning, checkedColors.warningSoft], [checkedColors.danger, checkedColors.dangerSoft]] as const)
      .every(([foreground, background]) => contrastRatio(foreground, background) >= 3);
}

export function parseThemeDefinition(value: unknown): ThemeDefinition | null {
  if (!validateThemeDefinition(value)) return null;
  const definition = value as ThemeDefinition;
  return {
    ...definition,
    name: { ...definition.name },
    colors: Object.fromEntries(APP_THEME_COLOR_KEYS.map((key) => [key, definition.colors[key].toLowerCase()])) as ThemeDefinition["colors"],
    ...(definition.terminalPalette ? {
      terminalPalette: Object.fromEntries(TERMINAL_COLOR_KEYS.map((key) => [key, definition.terminalPalette![key].toLowerCase()])) as TerminalPalette,
    } : {}),
  };
}

function validateThemeOverride(value: unknown): value is ThemeOverride {
  if (!isRecord(value)) return false;
  const candidate = value as Record<string, unknown>;
  const keys = ["colors", "fontFamily", "fontSize", "radius", "borderWidth", "density", "shadow"];
  if (!Object.keys(candidate).every((key) => keys.includes(key))) return false;
  if (candidate.colors !== undefined && (!isRecord(candidate.colors) || !Object.keys(candidate.colors).every((key) => APP_THEME_COLOR_KEYS.includes(key as AppThemeColorKey))
    || !Object.values(candidate.colors).every(isHexColor))) return false;
  return (candidate.fontFamily === undefined || ["system", "sans", "mono"].includes(candidate.fontFamily as string))
    && (candidate.fontSize === undefined || Number.isInteger(candidate.fontSize) && (candidate.fontSize as number) >= 12 && (candidate.fontSize as number) <= 18)
    && (candidate.radius === undefined || Number.isInteger(candidate.radius) && (candidate.radius as number) >= 0 && (candidate.radius as number) <= 12)
    && (candidate.borderWidth === undefined || candidate.borderWidth === 1 || candidate.borderWidth === 2)
    && (candidate.density === undefined || ["compact", "standard", "comfortable"].includes(candidate.density as string))
    && (candidate.shadow === undefined || ["none", "soft", "standard"].includes(candidate.shadow as string));
}

export function cloneDefaultAppThemeProfile(): AppThemeProfile {
  return { schemaVersion: 1, lightThemeId: BUILTIN_LIGHT_THEME_ID, darkThemeId: BUILTIN_DARK_THEME_ID, overrides: {} };
}

export function validateAppThemeProfile(value: unknown): value is AppThemeProfile {
  if (!exactKeys(value, ["schemaVersion", "lightThemeId", "darkThemeId", "overrides"])
    || byteLength(value) > APP_THEME_MAX_BYTES) return false;
  const candidate = value as Record<string, unknown>;
  if (candidate.schemaVersion !== APP_THEME_PROFILE_SCHEMA_VERSION
    || typeof candidate.lightThemeId !== "string" || typeof candidate.darkThemeId !== "string"
    || candidate.lightThemeId.length === 0 || candidate.lightThemeId.length > 256
    || candidate.darkThemeId.length === 0 || candidate.darkThemeId.length > 256 || !isRecord(candidate.overrides)) return false;
  const entries = Object.entries(candidate.overrides);
  return entries.length <= APP_THEME_MAX_OVERRIDES
    && entries.every(([key, override]) => key.length > 0 && key.length <= 256 && validateThemeOverride(override));
}

export function cloneAppThemeProfile(profile: AppThemeProfile): AppThemeProfile {
  return {
    schemaVersion: 1, lightThemeId: profile.lightThemeId, darkThemeId: profile.darkThemeId,
    overrides: Object.fromEntries(Object.entries(profile.overrides).map(([key, override]) => [key, {
      ...override, ...(override.colors ? { colors: { ...override.colors } } : {}),
    }])),
  };
}

export function pluginThemeKey(pluginId: string, definitionId: string): ThemeChoiceKey {
  return `plugin:${pluginId}:${definitionId}`;
}

export function themeWithOverride(definition: ThemeDefinition, override: ThemeOverride | undefined): ThemeDefinition {
  if (!override) return definition;
  return {
    ...definition,
    ...override,
    colors: { ...definition.colors, ...override.colors },
    ...(definition.terminalPalette ? { terminalPalette: { ...definition.terminalPalette } } : {}),
  };
}

/** Maps a validated definition to host-owned CSS custom properties only. */
export function themeStyles(definition: ThemeDefinition): Record<string, string> {
  const fontFamily = definition.fontFamily === "mono" ? "var(--nvx-font-mono)"
    : definition.fontFamily === "sans" ? "Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif"
      : "ui-sans-serif, -apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif";
  const density = definition.density === "compact" ? 0.875 : definition.density === "comfortable" ? 1.125 : 1;
  const shadow = definition.shadow === "none" ? "none"
    : definition.shadow === "soft" ? "0 2px 8px rgb(16 24 40 / 8%)"
      : "0 4px 12px rgb(16 24 40 / 8%)";
  const baseSpaces = { 1: 4, 2: 8, 3: 12, 4: 16, 5: 20, 6: 24, 8: 32, 10: 40 } as const;
  const scaledSpace = (value: number) => `${Math.round(value * density)}px`;
  const sizeDelta = definition.fontSize - 14;
  return {
    ...Object.fromEntries(APP_THEME_COLOR_KEYS.map((key) => [APP_THEME_COLOR_CSS_VARIABLES[key], definition.colors[key]])),
    "--nvx-font-sans": fontFamily,
    "--nvx-theme-font-size": `${definition.fontSize}px`,
    "--nvx-font-size-xs": `${12 + sizeDelta}px`,
    "--nvx-font-size-sm": `${13 + sizeDelta}px`,
    "--nvx-font-size-body": `${definition.fontSize}px`,
    "--nvx-font-size-md": `${16 + sizeDelta}px`,
    "--nvx-font-size-lg": `${20 + sizeDelta}px`,
    "--nvx-font-size-xl": `${24 + sizeDelta}px`,
    "--nvx-line-height-xs": `${18 + sizeDelta}px`,
    "--nvx-line-height-sm": `${20 + sizeDelta}px`,
    "--nvx-line-height-body": `${22 + sizeDelta}px`,
    "--nvx-line-height-md": `${24 + sizeDelta}px`,
    "--nvx-line-height-lg": `${28 + sizeDelta}px`,
    "--nvx-line-height-xl": `${32 + sizeDelta}px`,
    "--nvx-radius-sm": `${Math.max(0, definition.radius - 2)}px`,
    "--nvx-radius-md": `${definition.radius}px`,
    "--nvx-border-width": `${definition.borderWidth}px`,
    ...Object.fromEntries(Object.entries(baseSpaces).map(([key, value]) => [`--nvx-space-${key}`, scaledSpace(value)])),
    "--nvx-control-height-sm": scaledSpace(32),
    "--nvx-control-height-md": scaledSpace(40),
    "--nvx-control-height-touch": scaledSpace(44),
    "--nvx-shadow-toast": shadow,
    "--nvx-shadow-overlay": definition.shadow === "none" ? "none" : definition.shadow === "soft"
      ? "0 6px 18px rgb(16 24 40 / 10%)" : "0 12px 32px rgb(16 24 40 / 16%)",
  };
}
