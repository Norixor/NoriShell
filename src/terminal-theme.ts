export const TERMINAL_COLOR_KEYS = [
  "background",
  "foreground",
  "muted",
  "cursor",
  "selection",
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

export type TerminalColorKey = (typeof TERMINAL_COLOR_KEYS)[number];
export type TerminalPalette = Record<TerminalColorKey, string>;
export type TerminalPresetAppearance = "light" | "dark";

export interface TerminalPreset {
  id: string;
  appearance: TerminalPresetAppearance;
  nameKey: string;
  descriptionKey: string;
  palette: TerminalPalette;
}

export const TERMINAL_PALETTE_CSS_VARIABLES: Record<TerminalColorKey, string> = {
  background: "--nvx-color-terminal-bg",
  foreground: "--nvx-color-terminal-fg",
  muted: "--nvx-color-terminal-muted",
  cursor: "--nvx-color-terminal-cursor",
  selection: "--nvx-color-terminal-selection",
  black: "--nvx-color-terminal-ansi-black",
  red: "--nvx-color-terminal-ansi-red",
  green: "--nvx-color-terminal-ansi-green",
  yellow: "--nvx-color-terminal-ansi-yellow",
  blue: "--nvx-color-terminal-ansi-blue",
  magenta: "--nvx-color-terminal-ansi-magenta",
  cyan: "--nvx-color-terminal-ansi-cyan",
  white: "--nvx-color-terminal-ansi-white",
  brightBlack: "--nvx-color-terminal-ansi-bright-black",
  brightRed: "--nvx-color-terminal-ansi-bright-red",
  brightGreen: "--nvx-color-terminal-ansi-bright-green",
  brightYellow: "--nvx-color-terminal-ansi-bright-yellow",
  brightBlue: "--nvx-color-terminal-ansi-bright-blue",
  brightMagenta: "--nvx-color-terminal-ansi-bright-magenta",
  brightCyan: "--nvx-color-terminal-ansi-bright-cyan",
  brightWhite: "--nvx-color-terminal-ansi-bright-white",
};

export const LIGHT_TERMINAL_PALETTE: TerminalPalette = {
  background: "#f7f8fa",
  foreground: "#171a21",
  muted: "#525866",
  cursor: "#1f5fd2",
  selection: "#c9dafe",
  black: "#1f2430",
  red: "#b4232d",
  green: "#18723c",
  yellow: "#8b5b00",
  blue: "#1f5fd2",
  magenta: "#7a3eb1",
  cyan: "#0b6f7a",
  white: "#dce1e8",
  brightBlack: "#5b6472",
  brightRed: "#d13a45",
  brightGreen: "#25834b",
  brightYellow: "#a96416",
  brightBlue: "#3b75e8",
  brightMagenta: "#9858d0",
  brightCyan: "#168595",
  brightWhite: "#ffffff",
};

export const DARK_TERMINAL_PALETTE: TerminalPalette = {
  background: "#101217",
  foreground: "#f2f4f7",
  muted: "#b6bfcc",
  cursor: "#8aaeff",
  selection: "#33415f",
  black: "#1b1f27",
  red: "#e06c75",
  green: "#98c379",
  yellow: "#e5c07b",
  blue: "#61afef",
  magenta: "#c678dd",
  cyan: "#56b6c2",
  white: "#d7dae0",
  brightBlack: "#5c6370",
  brightRed: "#ff7b86",
  brightGreen: "#b3df8f",
  brightYellow: "#ffd58a",
  brightBlue: "#7cc4ff",
  brightMagenta: "#db91f0",
  brightCyan: "#73d4df",
  brightWhite: "#ffffff",
};

const SOLARIZED_LIGHT_TERMINAL_PALETTE: TerminalPalette = {
  background: "#fdf6e3",
  foreground: "#586e75",
  muted: "#5f747b",
  cursor: "#268bd2",
  selection: "#eee8d5",
  black: "#073642",
  red: "#dc322f",
  green: "#859900",
  yellow: "#b58900",
  blue: "#268bd2",
  magenta: "#d33682",
  cyan: "#2aa198",
  white: "#eee8d5",
  brightBlack: "#002b36",
  brightRed: "#cb4b16",
  brightGreen: "#586e75",
  brightYellow: "#657b83",
  brightBlue: "#839496",
  brightMagenta: "#6c71c4",
  brightCyan: "#93a1a1",
  brightWhite: "#fdf6e3",
};

const SOLARIZED_DARK_TERMINAL_PALETTE: TerminalPalette = {
  background: "#002b36",
  foreground: "#eee8d5",
  muted: "#93a1a1",
  cursor: "#93a1a1",
  selection: "#073642",
  black: "#073642",
  red: "#dc322f",
  green: "#859900",
  yellow: "#b58900",
  blue: "#268bd2",
  magenta: "#d33682",
  cyan: "#2aa198",
  white: "#eee8d5",
  brightBlack: "#002b36",
  brightRed: "#cb4b16",
  brightGreen: "#586e75",
  brightYellow: "#657b83",
  brightBlue: "#839496",
  brightMagenta: "#6c71c4",
  brightCyan: "#93a1a1",
  brightWhite: "#fdf6e3",
};

const NORD_TERMINAL_PALETTE: TerminalPalette = {
  background: "#2e3440",
  foreground: "#eceff4",
  muted: "#d8dee9",
  cursor: "#88c0d0",
  selection: "#434c5e",
  black: "#3b4252",
  red: "#bf616a",
  green: "#a3be8c",
  yellow: "#ebcb8b",
  blue: "#81a1c1",
  magenta: "#b48ead",
  cyan: "#88c0d0",
  white: "#e5e9f0",
  brightBlack: "#4c566a",
  brightRed: "#d57780",
  brightGreen: "#b1d196",
  brightYellow: "#f0d399",
  brightBlue: "#8fabc8",
  brightMagenta: "#c29bbc",
  brightCyan: "#9ad1dc",
  brightWhite: "#eceff4",
};

const GRUVBOX_DARK_TERMINAL_PALETTE: TerminalPalette = {
  background: "#282828",
  foreground: "#ebdbb2",
  muted: "#d5c4a1",
  cursor: "#fabd2f",
  selection: "#504945",
  black: "#282828",
  red: "#cc241d",
  green: "#98971a",
  yellow: "#d79921",
  blue: "#458588",
  magenta: "#b16286",
  cyan: "#689d6a",
  white: "#a89984",
  brightBlack: "#928374",
  brightRed: "#fb4934",
  brightGreen: "#b8bb26",
  brightYellow: "#fabd2f",
  brightBlue: "#83a598",
  brightMagenta: "#d3869b",
  brightCyan: "#8ec07c",
  brightWhite: "#ebdbb2",
};

export const TERMINAL_PRESETS = [
  {
    id: "norishell-light",
    appearance: "light",
    nameKey: "sshSettings.terminalAppearance.presets.norishellLight.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.norishellLight.description",
    palette: LIGHT_TERMINAL_PALETTE,
  },
  {
    id: "norishell-dark",
    appearance: "dark",
    nameKey: "sshSettings.terminalAppearance.presets.norishellDark.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.norishellDark.description",
    palette: DARK_TERMINAL_PALETTE,
  },
  {
    id: "solarized-light",
    appearance: "light",
    nameKey: "sshSettings.terminalAppearance.presets.solarizedLight.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.solarizedLight.description",
    palette: SOLARIZED_LIGHT_TERMINAL_PALETTE,
  },
  {
    id: "solarized-dark",
    appearance: "dark",
    nameKey: "sshSettings.terminalAppearance.presets.solarizedDark.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.solarizedDark.description",
    palette: SOLARIZED_DARK_TERMINAL_PALETTE,
  },
  {
    id: "nord",
    appearance: "dark",
    nameKey: "sshSettings.terminalAppearance.presets.nord.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.nord.description",
    palette: NORD_TERMINAL_PALETTE,
  },
  {
    id: "gruvbox-dark",
    appearance: "dark",
    nameKey: "sshSettings.terminalAppearance.presets.gruvboxDark.name",
    descriptionKey: "sshSettings.terminalAppearance.presets.gruvboxDark.description",
    palette: GRUVBOX_DARK_TERMINAL_PALETTE,
  },
] as const satisfies readonly TerminalPreset[];

export type TerminalPresetId = (typeof TERMINAL_PRESETS)[number]["id"];
export type TerminalThemeMode = "follow-app" | "custom" | TerminalPresetId;
export type LegacyTerminalThemeMode = "light" | "dark";

export interface ParsedTerminalAppearance {
  mode: TerminalThemeMode;
  customPalette: TerminalPalette;
  customName: string;
  hasCustomPalette: boolean;
}

export const DEFAULT_TERMINAL_FONT_SIZE = 13;
export const MIN_TERMINAL_FONT_SIZE = 10;
export const MAX_TERMINAL_FONT_SIZE = 28;
export const DEFAULT_TERMINAL_FONT_FAMILY = "Menlo";
export const DEFAULT_TERMINAL_FONT_WEIGHT = 400;
export const DEFAULT_TERMINAL_BOLD_FONT_WEIGHT = 700;
export const DEFAULT_TERMINAL_LINE_HEIGHT = 1;
export const DEFAULT_TERMINAL_LETTER_SPACING = 0;
export const MIN_TERMINAL_LINE_HEIGHT = 1;
export const MAX_TERMINAL_LINE_HEIGHT = 2;
export const MIN_TERMINAL_LETTER_SPACING = -2;
export const MAX_TERMINAL_LETTER_SPACING = 4;

export const TERMINAL_FONT_WEIGHTS = [300, 400, 500, 600, 700, 800, 900] as const;
export type TerminalFontWeight = (typeof TERMINAL_FONT_WEIGHTS)[number];
export type TerminalCursorStyle = "block" | "underline" | "bar";

const TERMINAL_FONT_FALLBACK = [
  "Monaco",
  "SF Mono",
  "SFMono-Regular",
  "Menlo",
  "Cascadia Mono",
  "Cascadia Code",
  "Consolas",
  "Liberation Mono",
  "ui-monospace",
  "monospace",
].join(", ");

export function parseTerminalFontSize(value: unknown): number {
  const parsed = typeof value === "number"
    ? value
    : typeof value === "string" && value.trim() !== ""
      ? Number(value)
      : Number.NaN;
  return Number.isInteger(parsed)
    && parsed >= MIN_TERMINAL_FONT_SIZE
    && parsed <= MAX_TERMINAL_FONT_SIZE
    ? parsed
    : DEFAULT_TERMINAL_FONT_SIZE;
}

export function parseTerminalFontWeight(
  value: unknown,
  fallback: TerminalFontWeight = DEFAULT_TERMINAL_FONT_WEIGHT,
): TerminalFontWeight {
  const parsed = typeof value === "number" ? value : Number(value);
  return TERMINAL_FONT_WEIGHTS.includes(parsed as TerminalFontWeight)
    ? parsed as TerminalFontWeight
    : fallback;
}

export function parseTerminalLineHeight(value: unknown): number {
  const parsed = Number(value);
  return Number.isFinite(parsed)
    && parsed >= MIN_TERMINAL_LINE_HEIGHT
    && parsed <= MAX_TERMINAL_LINE_HEIGHT
    ? Math.round(parsed * 10) / 10
    : DEFAULT_TERMINAL_LINE_HEIGHT;
}

export function parseTerminalLetterSpacing(value: unknown): number {
  const parsed = Number(value);
  return Number.isInteger(parsed)
    && parsed >= MIN_TERMINAL_LETTER_SPACING
    && parsed <= MAX_TERMINAL_LETTER_SPACING
    ? parsed
    : DEFAULT_TERMINAL_LETTER_SPACING;
}

export function parseTerminalCursorStyle(value: unknown): TerminalCursorStyle {
  return value === "underline" || value === "bar" ? value : "block";
}

export function normalizeTerminalFontFamily(value: unknown): string {
  if (typeof value !== "string") return DEFAULT_TERMINAL_FONT_FAMILY;
  const normalized = value.trim().replace(/\s+/g, " ");
  const containsControlCharacter = [...normalized].some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint < 32 || codePoint === 127;
  });
  if (
    normalized.length > 80
    || containsControlCharacter
    || /[,;{}()'"\\]/u.test(normalized)
  ) {
    return DEFAULT_TERMINAL_FONT_FAMILY;
  }
  return normalized;
}

export function terminalFontCssFamily(fontFamily: string): string {
  const normalized = normalizeTerminalFontFamily(fontFamily);
  return normalized ? `"${normalized}", ${TERMINAL_FONT_FALLBACK}` : TERMINAL_FONT_FALLBACK;
}

const HEX_COLOR = /^#[0-9a-f]{6}$/i;
const MAX_CUSTOM_NAME_LENGTH = 48;

export function isHexColor(value: unknown): value is string {
  return typeof value === "string" && HEX_COLOR.test(value);
}

export function cloneTerminalPalette(palette: TerminalPalette): TerminalPalette {
  return { ...palette };
}

export function parseTerminalPalette(value: unknown): TerminalPalette | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<Record<TerminalColorKey, unknown>>;
  if (!TERMINAL_COLOR_KEYS.every((key) => isHexColor(candidate[key]))) return null;
  return Object.fromEntries(
    TERMINAL_COLOR_KEYS.map((key) => [key, (candidate[key] as string).toLowerCase()]),
  ) as TerminalPalette;
}

export function isTerminalPresetId(value: unknown): value is TerminalPresetId {
  return TERMINAL_PRESETS.some((preset) => preset.id === value);
}

export function terminalPreset(id: TerminalPresetId): TerminalPreset {
  return TERMINAL_PRESETS.find((preset) => preset.id === id)!;
}

export function normalizeCustomTerminalPaletteName(value: unknown): string {
  if (typeof value !== "string") return "";
  return value.trim().slice(0, MAX_CUSTOM_NAME_LENGTH);
}

export function parseStoredTerminalAppearance(value: unknown): ParsedTerminalAppearance {
  const stored = value && typeof value === "object"
    ? value as Record<string, unknown>
    : {};
  const customPalette = parseTerminalPalette(stored.customTerminalPalette)
    ?? cloneTerminalPalette(DARK_TERMINAL_PALETTE);
  const customName = normalizeCustomTerminalPaletteName(stored.customTerminalPaletteName);

  if (
    stored.terminalAppearanceVersion === 5
    || stored.terminalAppearanceVersion === 4
    || stored.terminalAppearanceVersion === 3
  ) {
    const mode = stored.terminalThemeMode === "follow-app"
      || stored.terminalThemeMode === "custom"
      || isTerminalPresetId(stored.terminalThemeMode)
      ? stored.terminalThemeMode
      : "follow-app";
    return {
      mode,
      customPalette,
      customName,
      hasCustomPalette: mode === "custom" || customName.length > 0,
    };
  }

  if (stored.terminalAppearanceVersion === 2) {
    const legacyMode = stored.terminalThemeMode;
    const mode: TerminalThemeMode = legacyMode === "light"
      ? "norishell-light"
      : legacyMode === "dark"
        ? "norishell-dark"
        : legacyMode === "custom"
          ? "custom"
          : "follow-app";
    return {
      mode,
      customPalette,
      customName: "",
      hasCustomPalette: mode === "custom",
    };
  }

  return {
    mode: "follow-app",
    customPalette,
    customName: "",
    hasCustomPalette: false,
  };
}

export function resolveTerminalPalette(
  mode: TerminalThemeMode | LegacyTerminalThemeMode,
  appTheme: TerminalPresetAppearance,
  customPalette: TerminalPalette,
  appThemePalette: TerminalPalette | undefined = undefined,
): TerminalPalette {
  if (mode === "custom") return customPalette;
  if (mode === "follow-app") {
    return appThemePalette ?? (appTheme === "light" ? LIGHT_TERMINAL_PALETTE : DARK_TERMINAL_PALETTE);
  }
  if (mode === "light") return LIGHT_TERMINAL_PALETTE;
  if (mode === "dark") return DARK_TERMINAL_PALETTE;
  return terminalPreset(mode).palette;
}
