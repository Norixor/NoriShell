import { invoke } from "@tauri-apps/api/core";

import type { DesktopPreferences, DesktopPreferencesSnapshot, WireSequence } from "./generated/core-api";

export function getDesktopPreferences(): Promise<DesktopPreferencesSnapshot> {
  return invoke("desktop_preferences_get", { request: { meta: { requestId: crypto.randomUUID() } } });
}

export function replaceDesktopPreferences(preferences: DesktopPreferences, expectedRevision: WireSequence): Promise<DesktopPreferencesSnapshot> {
  return invoke("desktop_preferences_replace", {
    request: { meta: { requestId: crypto.randomUUID() }, preferences, expectedRevision },
  });
}
