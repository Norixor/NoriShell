import { describe, expect, it } from "vitest";

import { DEFAULT_TERMINAL_INTERACTION, XTERM_DEFAULT_WORD_SEPARATOR, parseStoredInteractionPreferences, validateInteractionPreferences, wordSeparatorForDoubleClickSelection } from "./interaction-preferences";

describe("terminal interaction preferences", () => {
  it("uses bounded defaults and the installed xterm word separator", () => {
    expect(DEFAULT_TERMINAL_INTERACTION).toMatchObject({ scrollback: 5_000, scrollSensitivity: 1, smoothScrollDuration: 0, doubleClickSelection: "word", copyOnSelect: false, rightClickBehavior: "menu", optionAsMetaLeft: false, optionAsMetaRight: false, backspaceMode: "del", bellMode: "off", linksEnabled: true, sshReconnectOnInput: true });
    expect(wordSeparatorForDoubleClickSelection("word")).toBe(XTERM_DEFAULT_WORD_SEPARATOR);
  });
  it("rejects partial, fractional, and out-of-range interaction data", () => {
    expect(validateInteractionPreferences(DEFAULT_TERMINAL_INTERACTION)).toBe(true);
    expect(validateInteractionPreferences({ ...DEFAULT_TERMINAL_INTERACTION, scrollback: 999 })).toBe(false);
    expect(validateInteractionPreferences({ ...DEFAULT_TERMINAL_INTERACTION, scrollSensitivity: 1.5 })).toBe(false);
    expect(validateInteractionPreferences({ ...DEFAULT_TERMINAL_INTERACTION, smoothScrollDuration: 50 })).toBe(false);
    expect(validateInteractionPreferences({ ...DEFAULT_TERMINAL_INTERACTION, futureMouseOption: true })).toBe(false);
    expect(validateInteractionPreferences({ scrollback: 5000 })).toBe(false);
  });
  it("keeps path and address punctuation available for their selection presets", () => {
    expect(wordSeparatorForDoubleClickSelection("path")).not.toContain("/");
    expect(wordSeparatorForDoubleClickSelection("address")).not.toContain(":");
    expect(wordSeparatorForDoubleClickSelection("address")).not.toContain("/");
  });
  it("migrates known older interaction data while rejecting unknown or malformed stored values", () => {
    const older = { scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 100, doubleClickSelection: "path" };
    expect(parseStoredInteractionPreferences(older)).toEqual({ ...older, copyOnSelect: false, rightClickBehavior: "menu", optionAsMetaLeft: false, optionAsMetaRight: false, backspaceMode: "del", bellMode: "off", linksEnabled: true, sshReconnectOnInput: true });
    expect(parseStoredInteractionPreferences({ ...older, copyOnSelect: "yes" })).toBeNull();
    expect(parseStoredInteractionPreferences({ ...older, sshReconnectOnInput: "yes" })).toBeNull();
    expect(parseStoredInteractionPreferences({ ...older, futureMouseOption: true })).toBeNull();
  });
});
