/**
  * Terminal interaction preferences are renderer-local configuration and never enter session payloads, output replay, or command history.
  * SSH input recovery only initiates the existing reconnect workflow and never retains input.
 */
export const XTERM_DEFAULT_WORD_SEPARATOR = " ()[]{}',`\"";

export type DoubleClickSelection = "word" | "path" | "address";
export type RightClickBehavior = "menu" | "paste";
export type TerminalBellMode = "off" | "visual" | "sound";
export type TerminalBackspaceMode = "del" | "bs";

export interface InteractionPreferences {
  scrollback: number;
  scrollSensitivity: number;
  smoothScrollDuration: 0 | 100 | 200;
  doubleClickSelection: DoubleClickSelection;
  copyOnSelect: boolean;
  rightClickBehavior: RightClickBehavior;
  /** macOS only: update xterm's public macOptionIsMeta before each physical Option-key event. */
  optionAsMetaLeft: boolean;
  optionAsMetaRight: boolean;
  backspaceMode: TerminalBackspaceMode;
  bellMode: TerminalBellMode;
  linksEnabled: boolean;
  /** When a failed SSH Pane receives input, reconnect first and discard that triggering input. */
  sshReconnectOnInput: boolean;
}

export const DEFAULT_TERMINAL_INTERACTION: Readonly<InteractionPreferences> = {
  scrollback: 5_000,
  scrollSensitivity: 1,
  smoothScrollDuration: 0,
  doubleClickSelection: "word",
  copyOnSelect: false,
  rightClickBehavior: "menu",
  optionAsMetaLeft: false,
  optionAsMetaRight: false,
  backspaceMode: "del",
  bellMode: "off",
  linksEnabled: true,
  sshReconnectOnInput: true,
};

const selectionValues = new Set<DoubleClickSelection>(["word", "path", "address"]);
const scrollDurationValues = new Set<InteractionPreferences["smoothScrollDuration"]>([0, 100, 200]);
const rightClickValues = new Set<RightClickBehavior>(["menu", "paste"]);
const backspaceValues = new Set<TerminalBackspaceMode>(["del", "bs"]);
const bellValues = new Set<TerminalBellMode>(["off", "visual", "sound"]);
const interactionKeys = ["scrollback", "scrollSensitivity", "smoothScrollDuration", "doubleClickSelection", "copyOnSelect", "rightClickBehavior", "optionAsMetaLeft", "optionAsMetaRight", "backspaceMode", "bellMode", "linksEnabled", "sshReconnectOnInput"] as const;

function hasValidExistingInteractionFields(candidate: Record<string, unknown>) {
  return typeof candidate.scrollback === "number"
    && Number.isInteger(candidate.scrollback)
    && candidate.scrollback >= 1_000
    && candidate.scrollback <= 100_000
    && typeof candidate.scrollSensitivity === "number"
    && Number.isInteger(candidate.scrollSensitivity)
    && candidate.scrollSensitivity >= 1
    && candidate.scrollSensitivity <= 10
    && scrollDurationValues.has(candidate.smoothScrollDuration as InteractionPreferences["smoothScrollDuration"])
    && selectionValues.has(candidate.doubleClickSelection as DoubleClickSelection);
}

export function validateInteractionPreferences(value: unknown): value is InteractionPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  const keys = Object.keys(candidate);
  return keys.length === interactionKeys.length
    && keys.every((key) => interactionKeys.includes(key as typeof interactionKeys[number]))
    && hasValidExistingInteractionFields(candidate)
    && typeof candidate.copyOnSelect === "boolean"
    && rightClickValues.has(candidate.rightClickBehavior as RightClickBehavior)
    && typeof candidate.optionAsMetaLeft === "boolean"
    && typeof candidate.optionAsMetaRight === "boolean"
    && backspaceValues.has(candidate.backspaceMode as TerminalBackspaceMode)
    && bellValues.has(candidate.bellMode as TerminalBellMode)
    && typeof candidate.linksEnabled === "boolean"
    && typeof candidate.sshReconnectOnInput === "boolean";
}

/** Add defaults only when legacy preferences lack new fields; reject unknown keys or any corrupted existing field. */
export function parseStoredInteractionPreferences(value: unknown): InteractionPreferences | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const candidate = value as Record<string, unknown>;
  if (!Object.keys(candidate).every((key) => interactionKeys.includes(key as typeof interactionKeys[number]))) return null;
  if (!hasValidExistingInteractionFields(candidate)) return null;
  if (candidate.copyOnSelect !== undefined && typeof candidate.copyOnSelect !== "boolean") return null;
  if (candidate.rightClickBehavior !== undefined && !rightClickValues.has(candidate.rightClickBehavior as RightClickBehavior)) return null;
  if (candidate.optionAsMetaLeft !== undefined && typeof candidate.optionAsMetaLeft !== "boolean") return null;
  if (candidate.optionAsMetaRight !== undefined && typeof candidate.optionAsMetaRight !== "boolean") return null;
  if (candidate.backspaceMode !== undefined && !backspaceValues.has(candidate.backspaceMode as TerminalBackspaceMode)) return null;
  if (candidate.bellMode !== undefined && !bellValues.has(candidate.bellMode as TerminalBellMode)) return null;
  if (candidate.linksEnabled !== undefined && typeof candidate.linksEnabled !== "boolean") return null;
  if (candidate.sshReconnectOnInput !== undefined && typeof candidate.sshReconnectOnInput !== "boolean") return null;
  return {
    scrollback: candidate.scrollback,
    scrollSensitivity: candidate.scrollSensitivity,
    smoothScrollDuration: candidate.smoothScrollDuration,
    doubleClickSelection: candidate.doubleClickSelection,
    copyOnSelect: candidate.copyOnSelect ?? DEFAULT_TERMINAL_INTERACTION.copyOnSelect,
    rightClickBehavior: candidate.rightClickBehavior ?? DEFAULT_TERMINAL_INTERACTION.rightClickBehavior,
    optionAsMetaLeft: candidate.optionAsMetaLeft ?? DEFAULT_TERMINAL_INTERACTION.optionAsMetaLeft,
    optionAsMetaRight: candidate.optionAsMetaRight ?? DEFAULT_TERMINAL_INTERACTION.optionAsMetaRight,
    backspaceMode: candidate.backspaceMode ?? DEFAULT_TERMINAL_INTERACTION.backspaceMode,
    bellMode: candidate.bellMode ?? DEFAULT_TERMINAL_INTERACTION.bellMode,
    linksEnabled: candidate.linksEnabled ?? DEFAULT_TERMINAL_INTERACTION.linksEnabled,
    sshReconnectOnInput: candidate.sshReconnectOnInput ?? DEFAULT_TERMINAL_INTERACTION.sshReconnectOnInput,
  } as InteractionPreferences;
}

/** xterm exposes wordSeparator through public ITerminalOptions; never read its internal OptionsService. */
export function wordSeparatorForDoubleClickSelection(selection: DoubleClickSelection): string {
  switch (selection) {
    case "word":
      // Match DEFAULT_OPTIONS.wordSeparator in the installed xterm 6.0.0.
      return XTERM_DEFAULT_WORD_SEPARATOR;
    case "path":
      // Keep /, ., :, @, and similar path characters together; parentheses and quotes remain separate Shell delimiters.
      return " ()[]{}'`\"";
    case "address":
      // URLs and addresses usually include path and query punctuation; split only on whitespace and surrounding quotes.
      return " \t\r\n'`\"<>";
  }
}
