import type { InteractionPreferences } from "./interaction-preferences";

/** Keyboard input is a local renderer preference; never persist it to Hosts, sessions, or workspaces. */
export type TerminalBackspaceMode = "del" | "bs";

export interface TerminalKeyboardPreferences {
  optionAsMetaLeft: boolean;
  optionAsMetaRight: boolean;
  backspaceMode: TerminalBackspaceMode;
}

export type HostKeyboardMode = "inherit" | "override";

export type HostKeyboardConfiguration =
  | { mode: "inherit" }
  | { mode: "override"; keyboard: TerminalKeyboardPreferences };

const backspaceModes = new Set<TerminalBackspaceMode>(["del", "bs"]);

export function keyboardPreferencesFromInteraction(interaction: InteractionPreferences): TerminalKeyboardPreferences {
  return {
    optionAsMetaLeft: interaction.optionAsMetaLeft,
    optionAsMetaRight: interaction.optionAsMetaRight,
    backspaceMode: interaction.backspaceMode,
  };
}

export function validateTerminalKeyboardPreferences(value: unknown): value is TerminalKeyboardPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  const keys = Object.keys(candidate);
  return keys.length === 3
    && keys.every((key) => ["optionAsMetaLeft", "optionAsMetaRight", "backspaceMode"].includes(key))
    && typeof candidate.optionAsMetaLeft === "boolean"
    && typeof candidate.optionAsMetaRight === "boolean"
    && backspaceModes.has(candidate.backspaceMode as TerminalBackspaceMode);
}

export function validateHostKeyboardConfiguration(value: unknown): value is HostKeyboardConfiguration {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  if (candidate.mode === "inherit") return Object.keys(candidate).length === 1;
  return candidate.mode === "override"
    && Object.keys(candidate).length === 2
    && validateTerminalKeyboardPreferences(candidate.keyboard);
}

/**
  * xterm exposes only one macOptionIsMeta switch. Its public key handler updates that switch before each key
  * from the most recently observed physical Option side, without reading xterm internal services.
 */
export class MacOptionKeyTracker {
  private leftDown = false;
  private rightDown = false;

  reset() {
    this.leftDown = false;
    this.rightDown = false;
  }

  observe(event: KeyboardEvent) {
    const pressed = event.type !== "keyup";
    if (event.code === "AltLeft") this.leftDown = pressed;
    else if (event.code === "AltRight") this.rightDown = pressed;
    else if (!event.altKey) this.reset();
  }

  shouldTreatOptionAsMeta(event: KeyboardEvent, preferences: TerminalKeyboardPreferences) {
    if (!event.altKey) return false;
    if (this.leftDown && !this.rightDown) return preferences.optionAsMetaLeft;
    if (this.rightDown && !this.leftDown) return preferences.optionAsMetaRight;
    // The first event can press Option before the view gains focus; fallback is safe only when both settings are enabled.
    return preferences.optionAsMetaLeft && preferences.optionAsMetaRight;
  }
}

export function isPlainBackspace(event: KeyboardEvent) {
  return event.type === "keydown"
    && event.key === "Backspace"
    && !event.altKey
    && !event.ctrlKey
    && !event.metaKey;
}
