import { defineStore } from "pinia";
import { ref } from "vue";

import { DEFAULT_HIGHLIGHT_RULES, validateHighlightConfiguration, type HighlightConfiguration, type HostHighlightConfiguration } from "../terminal/highlighting";
import { DEFAULT_TERMINAL_INTERACTION, parseStoredInteractionPreferences, validateInteractionPreferences, type InteractionPreferences } from "../terminal/interaction-preferences";
import {
  keyboardPreferencesFromInteraction,
  validateHostKeyboardConfiguration,
  type HostKeyboardConfiguration,
  type TerminalKeyboardPreferences,
} from "../terminal/keyboard-compatibility";

export const TERMINAL_PREFERENCES_KEY = "norishell.terminal-features.v1";
export type PasteWarningMode = "multiline" | "always" | "never";
export interface TerminalGlobalInteractionPreferences {
  interaction: InteractionPreferences;
  pasteWarning: PasteWarningMode;
}
export interface TerminalPreferences {
  version: 1;
  highlights: HighlightConfiguration;
  hostHighlights: Record<string, HostHighlightConfiguration>;
  /** Input display preferences are local bindings by HostId only; never include them in Host, Core, or preference exports. */
  hostKeyboard: Record<string, HostKeyboardConfiguration>;
  pasteWarning: PasteWarningMode;
  interaction: InteractionPreferences;
}

function isPasteWarningMode(value: unknown): value is PasteWarningMode {
  return ["multiline", "always", "never"].includes(value as PasteWarningMode);
}

function sameInteraction(left: InteractionPreferences, right: InteractionPreferences) {
  return left.scrollback === right.scrollback
    && left.scrollSensitivity === right.scrollSensitivity
    && left.smoothScrollDuration === right.smoothScrollDuration
    && left.doubleClickSelection === right.doubleClickSelection
    && left.copyOnSelect === right.copyOnSelect
    && left.rightClickBehavior === right.rightClickBehavior
    && left.optionAsMetaLeft === right.optionAsMetaLeft
    && left.optionAsMetaRight === right.optionAsMetaRight
    && left.backspaceMode === right.backspaceMode
    && left.bellMode === right.bellMode
    && left.linksEnabled === right.linksEnabled;
}

function defaults(): TerminalPreferences {
  return {
    version: 1,
    highlights: { enabled: false, rules: DEFAULT_HIGHLIGHT_RULES.map((rule) => ({ ...rule })) },
    hostHighlights: {},
    hostKeyboard: {},
    pasteWarning: "multiline",
    interaction: { ...DEFAULT_TERMINAL_INTERACTION },
  };
}

function load(): TerminalPreferences {
  const fallback = defaults();
  try {
    const raw = localStorage.getItem(TERMINAL_PREFERENCES_KEY);
    if (!raw || raw.length > 2_000_000) return fallback;
    const value = JSON.parse(raw) as Partial<TerminalPreferences>;
    if (value.version !== 1) return fallback;
    if (validateHighlightConfiguration(value.highlights)) fallback.highlights = value.highlights;
    if (isPasteWarningMode(value.pasteWarning)) fallback.pasteWarning = value.pasteWarning;
    // Existing v1 data has no interaction field; later fields add defaults only to known legacy objects.
    const interaction = parseStoredInteractionPreferences(value.interaction);
    if (interaction) fallback.interaction = interaction;
    if (value.hostHighlights && typeof value.hostHighlights === "object" && !Array.isArray(value.hostHighlights)) {
      for (const [hostId, config] of Object.entries(value.hostHighlights).slice(0, 2_000)) {
        if (/^[a-zA-Z0-9-]{1,80}$/.test(hostId) && validateHighlightConfiguration(config)
          && ["inherit", "custom", "disabled"].includes(config.mode)) fallback.hostHighlights[hostId] = config;
      }
    }
    if (value.hostKeyboard && typeof value.hostKeyboard === "object" && !Array.isArray(value.hostKeyboard)) {
      for (const [hostId, config] of Object.entries(value.hostKeyboard).slice(0, 2_000)) {
        // inherit has no persisted value; store only actual overrides to reduce Host-bound data.
        if (/^[a-zA-Z0-9-]{1,80}$/.test(hostId)
          && validateHostKeyboardConfiguration(config)
          && config.mode === "override") {
          fallback.hostKeyboard[hostId] = {
            mode: "override",
            keyboard: { ...config.keyboard },
          };
        }
      }
    }
  } catch { /* Corrupt display preferences fall back without modifying a session. */ }
  return fallback;
}

export const useTerminalPreferencesStore = defineStore("terminalPreferences", () => {
  const preferences = ref(load());
  function save(next: TerminalPreferences) {
    try { localStorage.setItem(TERMINAL_PREFERENCES_KEY, JSON.stringify(next)); }
    catch { return false; }
    preferences.value = next;
    return true;
  }
  function setPasteWarning(pasteWarning: PasteWarningMode) {
    if (!isPasteWarningMode(pasteWarning)) return false;
    return save({ ...preferences.value, pasteWarning });
  }
  function setInteraction(interaction: InteractionPreferences) {
    if (!validateInteractionPreferences(interaction)) return false;
    return save({ ...preferences.value, interaction: { ...interaction } });
  }
  /** For F04 import, compare and replace both global fields in one write; Host-bound preferences never participate. */
  function replaceGlobalInteraction(
    next: TerminalGlobalInteractionPreferences,
    expected: TerminalGlobalInteractionPreferences,
  ) {
    if (!validateInteractionPreferences(next.interaction)
      || !validateInteractionPreferences(expected.interaction)
      || !isPasteWarningMode(next.pasteWarning)
      || !isPasteWarningMode(expected.pasteWarning)) return false;
    const current: TerminalGlobalInteractionPreferences = {
      interaction: preferences.value.interaction,
      pasteWarning: preferences.value.pasteWarning,
    };
    if (current.pasteWarning !== expected.pasteWarning
      || !sameInteraction(current.interaction, expected.interaction)) return false;
    return save({
      ...preferences.value,
      interaction: { ...next.interaction },
      pasteWarning: next.pasteWarning,
    });
  }
  function setHighlights(config: HighlightConfiguration, hostId?: string, mode: HostHighlightConfiguration["mode"] = "custom") {
    if (!validateHighlightConfiguration(config)) return false;
    const copy = { enabled: config.enabled, rules: config.rules.map((rule) => ({ ...rule })) };
    if (!hostId) return save({ ...preferences.value, highlights: copy });
    if (!/^[a-zA-Z0-9-]{1,80}$/.test(hostId) || !["inherit", "custom", "disabled"].includes(mode)) return false;
    const hostHighlights = { ...preferences.value.hostHighlights };
    if (mode === "inherit") delete hostHighlights[hostId];
    else hostHighlights[hostId] = { ...copy, mode };
    if (Object.keys(hostHighlights).length > 2_000) return false;
    return save({ ...preferences.value, hostHighlights });
  }
  function resolvedHighlights(hostId?: string | null): HighlightConfiguration {
    const host = hostId ? preferences.value.hostHighlights[hostId] : undefined;
    if (!host || host.mode === "inherit") return preferences.value.highlights;
    return { enabled: host.mode === "custom" && host.enabled, rules: host.rules };
  }
  function setHostKeyboard(hostId: string, configuration: HostKeyboardConfiguration) {
    if (!/^[a-zA-Z0-9-]{1,80}$/.test(hostId) || !validateHostKeyboardConfiguration(configuration)) return false;
    const hostKeyboard = { ...preferences.value.hostKeyboard };
    if (configuration.mode === "inherit") delete hostKeyboard[hostId];
    else hostKeyboard[hostId] = { mode: "override", keyboard: { ...configuration.keyboard } };
    if (Object.keys(hostKeyboard).length > 2_000) return false;
    return save({ ...preferences.value, hostKeyboard });
  }
  function resolvedKeyboard(hostId?: string | null): TerminalKeyboardPreferences {
    const fallback = keyboardPreferencesFromInteraction(preferences.value.interaction);
    const host = hostId ? preferences.value.hostKeyboard[hostId] : undefined;
    return host?.mode === "override" ? { ...host.keyboard } : fallback;
  }
  return { preferences, setPasteWarning, setInteraction, replaceGlobalInteraction, setHighlights, resolvedHighlights, setHostKeyboard, resolvedKeyboard };
});
