import { ref } from "vue";

/**
 * Soft-keyboard helper keys for the phone shell. Ctrl and Alt are sticky: the next
 * character typed on the on-screen keyboard (or the next helper key) takes the
 * modifier, then the modifier clears. The state is renderer-only and never part
 * of a Session, a lease or persistence.
 */
export type ExtraKey =
  | "escape" | "tab" | "arrowUp" | "arrowDown" | "arrowLeft" | "arrowRight"
  | "home" | "end" | "pageUp" | "pageDown" | "pipe" | "tilde" | "slash" | "dash";

export const stickyCtrl = ref(false);
export const stickyAlt = ref(false);

export function toggleStickyCtrl() { stickyCtrl.value = !stickyCtrl.value; }
export function toggleStickyAlt() { stickyAlt.value = !stickyAlt.value; }
export function clearStickyModifiers() { stickyCtrl.value = false; stickyAlt.value = false; }

function takeStickyModifiers(): { ctrl: boolean; alt: boolean } {
  const taken = { ctrl: stickyCtrl.value, alt: stickyAlt.value };
  clearStickyModifiers();
  return taken;
}

/** The control byte for a printable key, or null when the key has no control form. */
export function controlCharacter(character: string): string | null {
  const upper = character.toUpperCase();
  if (upper.length !== 1) return null;
  const code = upper.charCodeAt(0);
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code & 0x1f); // @ A-Z [ \ ] ^ _
  if (character === "?") return "\x7f";
  if (character === " " || character === "2") return "\x00";
  if (/^[3-7]$/.test(character)) return String.fromCharCode(0x1b + (Number(character) - 3));
  if (character === "8") return "\x7f";
  return null;
}

/**
 * Applies pending sticky modifiers to data the on-screen keyboard produced. A
 * single typed character becomes its Ctrl/Alt form; any other data (escape
 * sequences from a physical keyboard, multi-character IME commits) passes through
 * unchanged. Either way a pending modifier is consumed, so it cannot leak into a
 * later keystroke the user did not intend.
 */
export function applyStickyModifiers(data: string): string {
  if (!stickyCtrl.value && !stickyAlt.value) return data;
  const { ctrl, alt } = takeStickyModifiers();
  if (Array.from(data).length !== 1 || data.charCodeAt(0) < 0x20) return data;
  let result = data;
  if (ctrl) result = controlCharacter(data) ?? data;
  if (alt) result = `\x1b${result}`;
  return result;
}

const CURSOR_FINAL: Partial<Record<ExtraKey, string>> = {
  arrowUp: "A", arrowDown: "B", arrowRight: "C", arrowLeft: "D", home: "H", end: "F",
};
const TILDE_CODE: Partial<Record<ExtraKey, string>> = { pageUp: "5", pageDown: "6" };
const LITERAL: Partial<Record<ExtraKey, string>> = { pipe: "|", tilde: "~", slash: "/", dash: "-", tab: "\t", escape: "\x1b" };

/**
 * The byte sequence an xterm-compatible terminal expects for a helper key, using
 * the xterm modifier parameter (2 = Shift, 3 = Alt, 5 = Ctrl, 7 = Ctrl+Alt). Cursor
 * keys without a modifier follow DECCKM: `ESC O x` in application mode, `ESC [ x`
 * otherwise. Pending sticky modifiers are consumed here as well.
 */
export function extraKeySequence(key: ExtraKey, applicationCursorKeys: boolean): string {
  const { ctrl, alt } = takeStickyModifiers();
  const modifier = 1 + (alt ? 2 : 0) + (ctrl ? 4 : 0);
  const literal = LITERAL[key];
  if (literal !== undefined) {
    if (key === "tab" || key === "escape") return alt ? `\x1b${literal}` : literal;
    let result = literal;
    if (ctrl) result = controlCharacter(literal) ?? literal;
    if (alt) result = `\x1b${result}`;
    return result;
  }
  const cursorFinal = CURSOR_FINAL[key];
  if (cursorFinal) {
    if (modifier > 1) return `\x1b[1;${modifier}${cursorFinal}`;
    return `${applicationCursorKeys ? "\x1bO" : "\x1b["}${cursorFinal}`;
  }
  const tildeCode = TILDE_CODE[key]!;
  return modifier > 1 ? `\x1b[${tildeCode};${modifier}~` : `\x1b[${tildeCode}~`;
}
