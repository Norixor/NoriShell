import type { ApplicationPreferences } from "./ui-transfer";

export const SECURE_WINDOW_APPEARANCE_KEY = "norishell.secure-appearance.v1";

export function publishSecureWindowAppearance(value: ApplicationPreferences) {
  try {
    localStorage.setItem(SECURE_WINDOW_APPEARANCE_KEY, JSON.stringify({
      locale: value.locale,
      themePreference: value.themePreference,
    }));
  } catch { /* A secondary window can still use the system appearance. */ }
}
