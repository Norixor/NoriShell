import { invoke } from "@tauri-apps/api/core";

import { BUILTIN_APP_THEMES, themeStyles, validateThemeDefinition, type ThemeAppearance, type ThemeDefinition } from "./app-theme";
import { hasTauriIpc, isMobileLayout } from "./platform-host";
import { parseThemePreference, resolveThemePreference, UI_PREFERENCES_KEY } from "./ui-preferences";

const PROJECTION_KEY = "norishell.app-theme.projection.v1";
type ThemeProjection = Record<ThemeAppearance, ThemeDefinition>;

/** Regular auxiliary windows consume only resolved appearance data; they never load plugins or their runtime. */
export function publishAppThemeProjection(projection: ThemeProjection) {
  if (!validateThemeDefinition(projection.light) || !validateThemeDefinition(projection.dark)) return;
  syncAndroidSystemBars(projection);
  try {
    const text = JSON.stringify(projection);
    if (localStorage.getItem(PROJECTION_KEY) !== text) localStorage.setItem(PROJECTION_KEY, text);
  } catch { /* A saved main-window theme never rolls back because auxiliary-window caching fails. */ }
}

let lastSystemBars = "";

/**
 * Android only: status/navigation bar icons and the color behind the bars follow
 * the app theme chosen in Settings, not just the system night mode. The main
 * document sets `data-theme` before it applies and publishes the theme.
 */
function syncAndroidSystemBars(projection: ThemeProjection) {
  if (!isMobileLayout() || !hasTauriIpc()) return;
  const appearance: ThemeAppearance = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
  const request = { dark: appearance === "dark", background: projection[appearance].colors.bgSurface.toLowerCase() };
  const key = JSON.stringify(request);
  if (key === lastSystemBars) return;
  lastSystemBars = key;
  // Bars keep their previous appearance on failure; theme projection and layout are unaffected,
  // and neither a synchronous throw nor a rejection may escape into the theme store.
  const failed = () => { if (lastSystemBars === key) lastSystemBars = ""; };
  try {
    Promise.resolve(invoke("android_set_system_bars", { request: { meta: { requestId: crypto.randomUUID() }, ...request } }))
      .catch(failed);
  } catch {
    failed();
  }
}

export function readAppThemeProjection(appearance: ThemeAppearance): ThemeDefinition {
  try {
    const text = localStorage.getItem(PROJECTION_KEY);
    if (!text || new TextEncoder().encode(text).byteLength > 64 * 1024) return BUILTIN_APP_THEMES[appearance];
    const parsed: unknown = JSON.parse(text);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return BUILTIN_APP_THEMES[appearance];
    const candidate = (parsed as Record<string, unknown>)[appearance];
    if (validateThemeDefinition(candidate) && candidate.appearance === appearance) return candidate;
  } catch { /* Use built-in appearance when the cache is corrupt. */ }
  return BUILTIN_APP_THEMES[appearance];
}

export function initializeAuxiliaryThemeAppearance() {
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  function apply() {
    let stored: { themePreference?: unknown; theme?: unknown } = {};
    try {
      const parsed: unknown = JSON.parse(localStorage.getItem(UI_PREFERENCES_KEY) ?? "{}");
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) stored = parsed;
    } catch { /* Use the host default. */ }
    const appearance = resolveThemePreference(parseThemePreference(stored.themePreference, stored.theme), media?.matches ?? false);
    const definition = readAppThemeProjection(appearance);
    document.documentElement.dataset.theme = appearance;
    for (const [key, value] of Object.entries(themeStyles(definition))) document.documentElement.style.setProperty(key, value);
  }
  const onStorage = (event: StorageEvent) => {
    if (event.key === null || event.key === PROJECTION_KEY || event.key === UI_PREFERENCES_KEY) apply();
  };
  apply();
  window.addEventListener("storage", onStorage);
  media?.addEventListener("change", apply);
  const dispose = () => {
    window.removeEventListener("storage", onStorage);
    media?.removeEventListener("change", apply);
    window.removeEventListener("pagehide", dispose);
  };
  window.addEventListener("pagehide", dispose, { once: true });
  return dispose;
}
