import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TERMINAL_PREFERENCES_KEY, useTerminalPreferencesStore } from "./terminalPreferences";
import { DEFAULT_TERMINAL_INTERACTION } from "../terminal/interaction-preferences";
import { DEFAULT_HIGHLIGHT_RULES } from "../terminal/highlighting";

describe("terminal preferences interaction", () => {
  beforeEach(() => { vi.restoreAllMocks(); localStorage.clear(); setActivePinia(createPinia()); });
  it("loads old v1 terminal feature data with interaction defaults", () => {
    localStorage.setItem(TERMINAL_PREFERENCES_KEY, JSON.stringify({ version: 1, pasteWarning: "always" }));
    const store = useTerminalPreferencesStore();
    expect(store.preferences.pasteWarning).toBe("always");
    expect(store.preferences.interaction).toEqual(DEFAULT_TERMINAL_INTERACTION);
  });
  it("rejects invalid interaction data and keeps the previous value if storage fails", () => {
    const store = useTerminalPreferencesStore();
    expect(store.setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, scrollback: 999 })).toBe(false);
    const storageSpy = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    expect(store.setInteraction({ ...DEFAULT_TERMINAL_INTERACTION, scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 100, doubleClickSelection: "path" })).toBe(false);
    expect(store.preferences.interaction.scrollback).toBe(5_000);
    storageSpy.mockRestore();
  });
  it("persists a complete interaction object under the existing v1 terminal features key", () => {
    const store = useTerminalPreferencesStore();
    const interaction = { ...DEFAULT_TERMINAL_INTERACTION, scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 200 as const, doubleClickSelection: "address" as const, copyOnSelect: true, rightClickBehavior: "paste" as const, sshReconnectOnInput: false };
    expect(store.setInteraction(interaction)).toBe(true);
    expect(JSON.parse(localStorage.getItem(TERMINAL_PREFERENCES_KEY) ?? "{}").interaction).toEqual(interaction);
  });

  it("uses bounded per-Host keyboard overrides and atomically replaces only global interaction fields", () => {
    const store = useTerminalPreferencesStore();
    const hostId = "host-a";
    const highlights = { enabled: true, rules: DEFAULT_HIGHLIGHT_RULES.map((rule) => ({ ...rule })) };
    expect(store.setHighlights(highlights)).toBe(true);
    expect(store.setHighlights(highlights, hostId)).toBe(true);
    expect(store.setHostKeyboard(hostId, {
      mode: "override",
      keyboard: { optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "bs" },
    })).toBe(true);
    expect(store.resolvedKeyboard(hostId)).toEqual({ optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "bs" });

    const expected = {
      // Imported object property order is not part of the concurrency guard.
      interaction: Object.fromEntries(Object.entries(store.preferences.interaction).reverse()) as typeof store.preferences.interaction,
      pasteWarning: store.preferences.pasteWarning,
    };
    const next = {
      interaction: { ...DEFAULT_TERMINAL_INTERACTION, scrollback: 9_000, scrollSensitivity: 3, bellMode: "visual" as const, sshReconnectOnInput: false },
      pasteWarning: "always" as const,
    };
    const hostHighlights = JSON.parse(JSON.stringify(store.preferences.hostHighlights));
    const hostKeyboard = JSON.parse(JSON.stringify(store.preferences.hostKeyboard));
    const globalHighlights = JSON.parse(JSON.stringify(store.preferences.highlights));

    expect(store.replaceGlobalInteraction(next, expected)).toBe(true);
    expect(store.preferences.interaction).toEqual(next.interaction);
    expect(store.preferences.pasteWarning).toBe("always");
    expect(store.preferences.interaction.sshReconnectOnInput).toBe(false);
    expect(store.preferences.hostHighlights).toEqual(hostHighlights);
    expect(store.preferences.hostKeyboard).toEqual(hostKeyboard);
    expect(store.preferences.highlights).toEqual(globalHighlights);
    expect(store.replaceGlobalInteraction(
      { interaction: { ...store.preferences.interaction, sshReconnectOnInput: true }, pasteWarning: store.preferences.pasteWarning },
      { interaction: { ...store.preferences.interaction, sshReconnectOnInput: true }, pasteWarning: store.preferences.pasteWarning },
    )).toBe(false);
    expect(store.replaceGlobalInteraction(expected, expected)).toBe(false);

    expect(store.setHostKeyboard(hostId, { mode: "inherit" })).toBe(true);
    expect(store.preferences.hostKeyboard[hostId]).toBeUndefined();
    expect(store.resolvedKeyboard(hostId)).toEqual({
      optionAsMetaLeft: next.interaction.optionAsMetaLeft,
      optionAsMetaRight: next.interaction.optionAsMetaRight,
      backspaceMode: next.interaction.backspaceMode,
    });
  });
});
