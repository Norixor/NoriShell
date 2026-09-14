// Regular and secure windows share this storage key for non-secret appearance preferences.
export const UI_PREFERENCES_KEY = "norishell.ui.preferences.v1";

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = Exclude<ThemePreference, "system">;

export function parseThemePreference(value: unknown, legacyTheme: unknown): ThemePreference {
  if (value === "light" || value === "dark" || value === "system") return value;
  return legacyTheme === "dark" ? "dark" : "light";
}

export function resolveThemePreference(
  preference: ThemePreference,
  systemPrefersDark: boolean,
): ResolvedTheme {
  if (preference === "system") return systemPrefersDark ? "dark" : "light";
  return preference;
}
