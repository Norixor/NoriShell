import { ref } from "vue";

import { i18n, resolveLocale } from "./locales";
import {
  parseThemePreference,
  resolveThemePreference,
  UI_PREFERENCES_KEY,
} from "./ui-preferences";
import { disableDefaultWebviewContextMenu } from "./webview-context-menu";
import { SECURE_WINDOW_APPEARANCE_KEY } from "./secure-window-appearance";
import { applyUiZoom, isUiZoom, type UiZoom } from "./ui-zoom";

let removeSystemThemeListener: (() => void) | undefined;

// Read only non-secret appearance preferences; never load the main-window store or any plugin runtime.
export function applySecureWindowAppearance() {
  let stored: { locale?: unknown; theme?: unknown; themePreference?: unknown } = {};
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(SECURE_WINDOW_APPEARANCE_KEY)
      ?? localStorage.getItem(UI_PREFERENCES_KEY) ?? "{}");
    if (parsed && typeof parsed === "object") stored = parsed;
  } catch { /* Corrupt or unavailable preferences fall back to application defaults. */ }
  i18n.global.locale.value = resolveLocale(stored.locale === "en" || stored.locale === "zh-CN" ? stored.locale : "system");
  document.documentElement.lang = i18n.global.locale.value;
  const themePreference = parseThemePreference(stored.themePreference, stored.theme);
  removeSystemThemeListener?.();
  removeSystemThemeListener = undefined;

  if (themePreference === "system" && window.matchMedia) {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const applySystemTheme = () => {
      document.documentElement.dataset.theme = resolveThemePreference("system", mediaQuery.matches);
    };
    applySystemTheme();
    mediaQuery.addEventListener("change", applySystemTheme);
    removeSystemThemeListener = () => mediaQuery.removeEventListener("change", applySystemTheme);
    return;
  }

  document.documentElement.dataset.theme = resolveThemePreference(themePreference, false);
}

export function initializeSecureWindowAppearance() {
  disableDefaultWebviewContextMenu();
  applySecureWindowAppearance();
  const onStorage = (event: StorageEvent) => {
    if (event.key === SECURE_WINDOW_APPEARANCE_KEY || event.key === UI_PREFERENCES_KEY || event.key === null) applySecureWindowAppearance();
  };
  const dispose = () => {
    window.removeEventListener("storage", onStorage);
    window.removeEventListener("languagechange", applySecureWindowAppearance);
    window.removeEventListener("pagehide", dispose);
    removeSystemThemeListener?.();
    removeSystemThemeListener = undefined;
  };
  window.addEventListener("storage", onStorage);
  window.addEventListener("languagechange", applySecureWindowAppearance);
  window.addEventListener("pagehide", dispose, { once: true });
  return dispose;
}

function storedUiZoom(): UiZoom | null {
  for (const key of [SECURE_WINDOW_APPEARANCE_KEY, UI_PREFERENCES_KEY]) {
    try {
      const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "null");
      const value = parsed && typeof parsed === "object" ? (parsed as { uiZoom?: unknown }).uiZoom : undefined;
      if (isUiZoom(value)) return value;
    } catch { /* Unreadable preferences keep the WebView's default scale. */ }
  }
  return null;
}

/** Scale applied to this secondary window; 1 unless `initializeSecondaryWindowZoom` applied a UI zoom. */
export const secondaryWindowZoom = ref(1);

/**
 * Follows the main window's UI zoom in a resizable secondary window so its Header
 * and content match. Fixed-size protected windows keep their designed scale.
 */
export function initializeSecondaryWindowZoom() {
  let applied: UiZoom | null = null;
  const apply = () => {
    const zoom = storedUiZoom();
    if (zoom === null || zoom === applied) return;
    applied = zoom;
    void applyUiZoom(zoom)
      .then(() => { secondaryWindowZoom.value = zoom / 100; })
      .catch(() => { applied = null; });
  };
  apply();
  const onStorage = (event: StorageEvent) => {
    if (event.key === SECURE_WINDOW_APPEARANCE_KEY || event.key === UI_PREFERENCES_KEY || event.key === null) apply();
  };
  window.addEventListener("storage", onStorage);
  window.addEventListener("pagehide", () => window.removeEventListener("storage", onStorage), { once: true });
}
