import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { corePreferencesEnabled, saveApplicationPreferences } from "../core-api/application-preferences";

import {
  SHORTCUT_COMMANDS,
  SHORTCUT_PROFILE_VERSION,
  createDefaultShortcutBindings,
  findShortcutConflicts,
  getShortcutCommand,
  parseShortcutProfile,
  serializeShortcutProfile,
  validateShortcutBinding,
  type ShortcutBinding,
  type ShortcutBindings,
  type ShortcutCommandId,
  type ShortcutPlatform,
  type ShortcutProfile,
  type ShortcutValidationError,
} from "../shortcuts";

export const SHORTCUT_PREFERENCES_KEY = "norishell.shortcuts.v1";

export type ShortcutSaveResult =
  | { ok: true }
  | { ok: false; reason: "not-found" | "conflict" | "storage-error" | ShortcutValidationError; conflicts?: ShortcutCommandId[] };

export type ShortcutImportResult =
  | { ok: true }
  | { ok: false; reason: "invalid-json" | "invalid-profile" | "too-large" | "storage-error" | "conflict" };

function defaultsProfile(): ShortcutProfile {
  return {
    version: SHORTCUT_PROFILE_VERSION,
    bindings: {
      macos: createDefaultShortcutBindings("macos"),
      windows: createDefaultShortcutBindings("windows"),
    },
  };
}

function loadProfile() {
  try {
    const stored = localStorage.getItem(SHORTCUT_PREFERENCES_KEY);
    if (!stored) return defaultsProfile();
    const parsed = parseShortcutProfile(stored);
    return parsed.ok ? parsed.profile : defaultsProfile();
  } catch {
    return defaultsProfile();
  }
}

function cloneBindings(bindings: ShortcutBindings): ShortcutBindings {
  return { ...bindings };
}

export const useShortcutsStore = defineStore("shortcuts", () => {
  const initial = loadProfile();
  const macosBindings = ref<ShortcutBindings>(cloneBindings(initial.bindings.macos));
  const windowsBindings = ref<ShortcutBindings>(cloneBindings(initial.bindings.windows));

  const profile = computed<ShortcutProfile>(() => ({
    version: SHORTCUT_PROFILE_VERSION,
    bindings: {
      macos: cloneBindings(macosBindings.value),
      windows: cloneBindings(windowsBindings.value),
    },
  }));

  function hydrateCorePreferences(next: ShortcutProfile) {
    macosBindings.value = cloneBindings(next.bindings.macos);
    windowsBindings.value = cloneBindings(next.bindings.windows);
  }

  function bindingsFor(platform: ShortcutPlatform) {
    return platform === "macos" ? macosBindings.value : windowsBindings.value;
  }

  function persist(next: ShortcutProfile) {
    try {
      localStorage.setItem(SHORTCUT_PREFERENCES_KEY, serializeShortcutProfile(next));
      return true;
    } catch {
      return false;
    }
  }

  async function updatePlatform(platform: ShortcutPlatform, nextBindings: ShortcutBindings) {
    const next: ShortcutProfile = {
      version: SHORTCUT_PROFILE_VERSION,
      bindings: {
        macos: platform === "macos" ? cloneBindings(nextBindings) : cloneBindings(macosBindings.value),
        windows: platform === "windows" ? cloneBindings(nextBindings) : cloneBindings(windowsBindings.value),
      },
    };
    if (corePreferencesEnabled()) {
      if (!await saveApplicationPreferences("shortcuts", next, profile.value)) return false;
    } else if (!persist(next)) return false;
    if (platform === "macos") macosBindings.value = next.bindings.macos;
    else windowsBindings.value = next.bindings.windows;
    return true;
  }

  async function setBinding(
    platform: ShortcutPlatform,
    commandId: ShortcutCommandId,
    value: ShortcutBinding,
  ): Promise<ShortcutSaveResult> {
    const command = getShortcutCommand(commandId);
    if (!command) return { ok: false, reason: "not-found" };
    const validation = validateShortcutBinding(value, platform, command.scope);
    if (validation.error) return { ok: false, reason: validation.error };
    const current = bindingsFor(platform);
    const conflicts = findShortcutConflicts(current, commandId, validation.binding);
    if (conflicts.length) {
      return { ok: false, reason: "conflict", conflicts: conflicts.map((item) => item.id) };
    }
    const next = { ...current, [commandId]: validation.binding };
    if (!await updatePlatform(platform, next)) return { ok: false, reason: "storage-error" };
    return { ok: true };
  }

  async function resetBinding(platform: ShortcutPlatform, commandId: ShortcutCommandId): Promise<ShortcutSaveResult> {
    const defaults = createDefaultShortcutBindings(platform);
    return setBinding(platform, commandId, defaults[commandId]);
  }

  async function resetAll(platform: ShortcutPlatform): Promise<ShortcutSaveResult> {
    const next = createDefaultShortcutBindings(platform);
    if (!await updatePlatform(platform, next)) return { ok: false, reason: "storage-error" };
    return { ok: true };
  }

  function exportProfile() {
    return serializeShortcutProfile(profile.value);
  }

  async function importProfile(text: string): Promise<ShortcutImportResult> {
    const parsed = parseShortcutProfile(text);
    if (!parsed.ok) return { ok: false, reason: parsed.error };
    if (corePreferencesEnabled()) {
      if (!await saveApplicationPreferences("shortcuts", parsed.profile, profile.value)) return { ok: false, reason: "storage-error" };
    } else if (!persist(parsed.profile)) return { ok: false, reason: "storage-error" };
    hydrateCorePreferences(parsed.profile);
    return { ok: true };
  }

  return {
    macosBindings,
    windowsBindings,
    profile,
    hydrateCorePreferences,
    commands: SHORTCUT_COMMANDS,
    bindingsFor,
    setBinding,
    resetBinding,
    resetAll,
    exportProfile,
    importProfile,
  };
});
