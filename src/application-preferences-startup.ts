import { setActivePinia, type Pinia } from "pinia";

import type { ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot } from "./core-api/generated/core-api";
import {
  corePreferencesEnabled,
  getApplicationPreferences,
  installApplicationPreferencesSnapshot,
  replaceApplicationPreferences,
} from "./core-api/application-preferences";
import { SECURE_WINDOW_APPEARANCE_KEY } from "./secure-window-appearance";
import { createPreferenceAdapters } from "./preference-adapters";
import { useSftpPreferencesStore } from "./stores/sftpPreferences";
import { useShortcutsStore } from "./stores/shortcuts";
import { useTerminalPreferencesStore } from "./stores/terminalPreferences";
import { useUiStore } from "./stores/ui";
import type { ShortcutProfile } from "./shortcuts";
import type { HighlightConfiguration } from "./terminal/highlighting";
import type { TerminalGlobalInteractionPreferences } from "./stores/terminalPreferences";
import type { SftpBrowserPreferences } from "./stores/sftpPreferences";
import { validateApplicationPreferences, type ApplicationPreferences, type AppearancePreferences } from "./ui-transfer";

const groups: ApplicationPreferenceGroupId[] = ["application", "appearance", "interaction", "highlights", "shortcuts", "files"];

export async function initializeApplicationPreferences(pinia: Pinia) {
  if (!corePreferencesEnabled()) return;
  setActivePinia(pinia);
  const adapters = createPreferenceAdapters();
  // Groups are independent Core records, so they load concurrently; any failure still fails startup.
  const loadGroup = async (group: ApplicationPreferenceGroupId): Promise<ApplicationPreferencesSnapshot> => {
    const adapter = adapters.find((candidate) => candidate.id === group);
    if (!adapter) throw new Error(`Missing preference adapter: ${group}`);
    let snapshot = await getApplicationPreferences(group);
    if (snapshot.group !== group || (snapshot.revision === null) !== (snapshot.value === null)) {
      throw new Error(`Invalid Core preference snapshot: ${group}`);
    }
    if (snapshot.revision === null) {
      // Stores were constructed from the existing local keys before any default could be persisted.
      const legacy = group === "application" ? useUiStore(pinia).legacyApplicationPreferences() : await adapter.read();
      if (!adapter.validate(legacy)) throw new Error(`Invalid local preference: ${group}`);
      try { snapshot = await replaceApplicationPreferences(group, legacy, null); }
      catch {
        // A concurrent window may have initialized the group first; Core remains authoritative.
        snapshot = await getApplicationPreferences(group);
      }
    }
    if (snapshot.group !== group || snapshot.revision === null || !adapter.validate(snapshot.value)) {
      throw new Error(`Invalid Core preference value: ${group}`);
    }
    return snapshot;
  };
  const loaded = new Map(await Promise.all(groups.map(async (group) => [group, await loadGroup(group)] as const)));
  for (const snapshot of loaded.values()) installApplicationPreferencesSnapshot(snapshot);
  const value = <T>(group: ApplicationPreferenceGroupId) => loaded.get(group)!.value as T;
  useUiStore(pinia).hydrateCorePreferences(value<ApplicationPreferences>("application"), value<AppearancePreferences>("appearance"));
  useTerminalPreferencesStore(pinia).hydrateCorePreferences(
    value<TerminalGlobalInteractionPreferences>("interaction"), value<HighlightConfiguration>("highlights"),
  );
  useShortcutsStore(pinia).hydrateCorePreferences(value<ShortcutProfile>("shortcuts"));
  useSftpPreferencesStore(pinia).hydrateCorePreferences(value<{ browser: SftpBrowserPreferences; rememberLastDirectory: boolean }>("files"));
}

export function observeApplicationPreferenceProjection(pinia: Pinia) {
  let generation = 0;
  const onStorage = (event: StorageEvent) => {
    if (event.key !== SECURE_WINDOW_APPEARANCE_KEY) return;
    const current = ++generation;
    void getApplicationPreferences("application").then(async (snapshot) => {
      if (current !== generation || snapshot.group !== "application" || snapshot.revision === null
        || !validateApplicationPreferences(snapshot.value)) return;
      const ui = useUiStore(pinia);
      installApplicationPreferencesSnapshot(snapshot);
      ui.hydrateCorePreferences(snapshot.value, ui.appearancePreferences());
      if (ui.appliedUiZoom !== snapshot.value.uiZoom) await ui.setUiZoom(snapshot.value.uiZoom, false);
    }).catch(() => { /* An unavailable Core leaves the last valid window projection in place. */ });
  };
  window.addEventListener("storage", onStorage);
  return () => {
    generation++;
    window.removeEventListener("storage", onStorage);
  };
}
