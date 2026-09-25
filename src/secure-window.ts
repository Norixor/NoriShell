import { i18n, resolveLocale } from "./locales";
import {
  parseThemePreference,
  resolveThemePreference,
  UI_PREFERENCES_KEY,
} from "./ui-preferences";
import { disableDefaultWebviewContextMenu } from "./webview-context-menu";

let removeSystemThemeListener: (() => void) | undefined;

// Read only language and theme; never load the main-window store or any plugin runtime.
export function applySecureWindowAppearance() {
  let stored: { locale?: unknown; theme?: unknown; themePreference?: unknown } = {};
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY) ?? "{}");
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
    if (event.key === UI_PREFERENCES_KEY || event.key === null) applySecureWindowAppearance();
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
