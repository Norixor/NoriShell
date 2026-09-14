import { describe, expect, it } from "vitest";

import {
  MacOptionKeyTracker,
  isPlainBackspace,
  keyboardPreferencesFromInteraction,
  validateHostKeyboardConfiguration,
  validateTerminalKeyboardPreferences,
} from "./keyboard-compatibility";
import { DEFAULT_TERMINAL_INTERACTION } from "./interaction-preferences";

function key(type: "keydown" | "keyup", init: KeyboardEventInit) {
  return new KeyboardEvent(type, init);
}

describe("terminal keyboard compatibility", () => {
  const keyboard = { optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "bs" as const };

  it("keeps the global keyboard fields small and validates host overrides strictly", () => {
    expect(keyboardPreferencesFromInteraction(DEFAULT_TERMINAL_INTERACTION)).toEqual({
      optionAsMetaLeft: false,
      optionAsMetaRight: false,
      backspaceMode: "del",
    });
    expect(validateTerminalKeyboardPreferences(keyboard)).toBe(true);
    expect(validateTerminalKeyboardPreferences({ ...keyboard, extra: true })).toBe(false);
    expect(validateHostKeyboardConfiguration({ mode: "inherit" })).toBe(true);
    expect(validateHostKeyboardConfiguration({ mode: "override", keyboard })).toBe(true);
    expect(validateHostKeyboardConfiguration({ mode: "override" })).toBe(false);
  });

  it("uses the observed physical Option side and fails closed before a side is known", () => {
    const tracker = new MacOptionKeyTracker();
    expect(tracker.shouldTreatOptionAsMeta(key("keydown", { key: "e", altKey: true }), keyboard)).toBe(false);

    tracker.observe(key("keydown", { code: "AltLeft", key: "Alt", altKey: true }));
    expect(tracker.shouldTreatOptionAsMeta(key("keydown", { key: "e", altKey: true }), keyboard)).toBe(true);

    tracker.observe(key("keyup", { code: "AltLeft", key: "Alt", altKey: false }));
    tracker.observe(key("keydown", { code: "AltRight", key: "Alt", altKey: true }));
    expect(tracker.shouldTreatOptionAsMeta(key("keydown", { key: "e", altKey: true }), keyboard)).toBe(false);

    tracker.reset();
    expect(tracker.shouldTreatOptionAsMeta(key("keydown", { key: "e", altKey: true }), keyboard)).toBe(false);
  });

  it("only remaps an unmodified keydown Backspace", () => {
    expect(isPlainBackspace(key("keydown", { key: "Backspace" }))).toBe(true);
    expect(isPlainBackspace(key("keyup", { key: "Backspace" }))).toBe(false);
    expect(isPlainBackspace(key("keydown", { key: "Backspace", ctrlKey: true }))).toBe(false);
    expect(isPlainBackspace(key("keydown", { key: "Backspace", altKey: true }))).toBe(false);
  });
});
