import { computed, ref } from "vue";
import { defineStore } from "pinia";

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

  function updatePlatform(platform: ShortcutPlatform, nextBindings: ShortcutBindings) {
    const next: ShortcutProfile = {
      version: SHORTCUT_PROFILE_VERSION,
      bindings: {
        macos: platform === "macos" ? cloneBindings(nextBindings) : cloneBindings(macosBindings.value),
        windows: platform === "windows" ? cloneBindings(nextBindings) : cloneBindings(windowsBindings.value),
      },
    };
    if (!persist(next)) return false;
    if (platform === "macos") macosBindings.value = next.bindings.macos;
    else windowsBindings.value = next.bindings.windows;
    return true;
  }

  function setBinding(
    platform: ShortcutPlatform,
    commandId: ShortcutCommandId,
    value: ShortcutBinding,
  ): ShortcutSaveResult {
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
    if (!updatePlatform(platform, next)) return { ok: false, reason: "storage-error" };
    return { ok: true };
  }

  function resetBinding(platform: ShortcutPlatform, commandId: ShortcutCommandId): ShortcutSaveResult {
    const defaults = createDefaultShortcutBindings(platform);
    return setBinding(platform, commandId, defaults[commandId]);
  }

  function resetAll(platform: ShortcutPlatform): ShortcutSaveResult {
    const next = createDefaultShortcutBindings(platform);
    if (!updatePlatform(platform, next)) return { ok: false, reason: "storage-error" };
    return { ok: true };
  }

  function exportProfile() {
    return serializeShortcutProfile(profile.value);
  }

  function importProfile(text: string): ShortcutImportResult {
    const parsed = parseShortcutProfile(text);
    if (!parsed.ok) return { ok: false, reason: parsed.error };
    if (!persist(parsed.profile)) return { ok: false, reason: "storage-error" };
    macosBindings.value = cloneBindings(parsed.profile.bindings.macos);
    windowsBindings.value = cloneBindings(parsed.profile.bindings.windows);
    return { ok: true };
  }

  return {
    macosBindings,
    windowsBindings,
    profile,
    commands: SHORTCUT_COMMANDS,
    bindingsFor,
    setBinding,
    resetBinding,
    resetAll,
    exportProfile,
    importProfile,
  };
});
