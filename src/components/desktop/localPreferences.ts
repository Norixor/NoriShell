import { reactive, readonly } from "vue";
import { detectDesktopPlatform } from "../../platform";

/**
 * Device-local remote desktop display preferences. They describe this machine's keyboard and
 * screen, so they are never part of a DesktopProfile, Core preferences or sync bundles; each
 * Desktop Tab WebView shares them through localStorage and follows changes made in another Tab.
 */
export const DESKTOP_LOCAL_PREFERENCES_KEY = "norishell.desktop-local.v1";
export interface DesktopLocalPreferences {
  /** macOS only: send ⌘ as the remote Control key. */
  commandAsControl: boolean;
  /** Request the remote desktop at device pixels and show it 1:1 in actual-size mode. */
  hiDpi: boolean;
}
const defaults = (): DesktopLocalPreferences => ({ commandAsControl: false, hiDpi: false });

function load(): DesktopLocalPreferences {
  const value = defaults();
  try {
    const raw = localStorage.getItem(DESKTOP_LOCAL_PREFERENCES_KEY);
    if (!raw || raw.length > 4096) return value;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || (parsed as { version?: unknown }).version !== 1) return value;
    const stored = parsed as Partial<Record<keyof DesktopLocalPreferences, unknown>>;
    if (typeof stored.commandAsControl === "boolean") value.commandAsControl = stored.commandAsControl;
    if (typeof stored.hiDpi === "boolean") value.hiDpi = stored.hiDpi;
  } catch { /* Corrupt or unavailable storage keeps the defaults without affecting a session. */ }
  return value;
}

const state = reactive(load());
let listening = false;
function listen() {
  if (listening || typeof window === "undefined") return;
  listening = true;
  window.addEventListener("storage", (event) => {
    if (event.key !== null && event.key !== DESKTOP_LOCAL_PREFERENCES_KEY) return;
    Object.assign(state, load());
  });
}

export function useDesktopLocalPreferences() {
  listen();
  function update(patch: Partial<DesktopLocalPreferences>) {
    const next = { ...state, ...patch };
    try { localStorage.setItem(DESKTOP_LOCAL_PREFERENCES_KEY, JSON.stringify({ version: 1, ...next })); }
    catch { /* The preference still applies to this WebView for the current run. */ }
    Object.assign(state, next);
  }
  return {
    preferences: readonly(state),
    /** The ⌘ mapping only exists on macOS keyboards. */
    commandAsControlAvailable: detectDesktopPlatform() === "macos",
    setCommandAsControl: (value: boolean) => update({ commandAsControl: value }),
    setHiDpi: (value: boolean) => update({ hiDpi: value }),
  };
}

/** Test hook: reloads state from storage after the test replaced it. */
export function reloadDesktopLocalPreferences() { Object.assign(state, load()); }
