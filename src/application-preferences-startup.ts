import { setActivePinia, type Pinia } from "pinia";

import type { ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot } from "./core-api/generated/core-api";
import {
  corePreferencesEnabled,
  getApplicationPreferences,
  installApplicationPreferencesSnapshot,
  replaceApplicationPreferences,
} from "./core-api/application-preferences";
import { createPreferenceAdapters } from "./preference-adapters";
import { useSftpPreferencesStore } from "./stores/sftpPreferences";
import { useShortcutsStore } from "./stores/shortcuts";
import { useTerminalPreferencesStore } from "./stores/terminalPreferences";
import { useUiStore } from "./stores/ui";
import type { ShortcutProfile } from "./shortcuts";
import type { HighlightConfiguration } from "./terminal/highlighting";
import type { TerminalGlobalInteractionPreferences } from "./stores/terminalPreferences";
import type { SftpBrowserPreferences } from "./stores/sftpPreferences";
import type { ApplicationPreferences, AppearancePreferences } from "./ui-transfer";

const groups: ApplicationPreferenceGroupId[] = ["application", "appearance", "interaction", "highlights", "shortcuts", "files"];

export async function initializeApplicationPreferences(pinia: Pinia) {
  if (!corePreferencesEnabled()) return;
  setActivePinia(pinia);
  const adapters = createPreferenceAdapters();
  const loaded = new Map<ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot>();
  for (const group of groups) {
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
    loaded.set(group, snapshot);
  }
  for (const snapshot of loaded.values()) installApplicationPreferencesSnapshot(snapshot);
  const value = <T>(group: ApplicationPreferenceGroupId) => loaded.get(group)!.value as T;
  useUiStore(pinia).hydrateCorePreferences(value<ApplicationPreferences>("application"), value<AppearancePreferences>("appearance"));
  useTerminalPreferencesStore(pinia).hydrateCorePreferences(
    value<TerminalGlobalInteractionPreferences>("interaction"), value<HighlightConfiguration>("highlights"),
  );
  useShortcutsStore(pinia).hydrateCorePreferences(value<ShortcutProfile>("shortcuts"));
  useSftpPreferencesStore(pinia).hydrateCorePreferences(value<{ browser: SftpBrowserPreferences; rememberLastDirectory: boolean }>("files"));
}
