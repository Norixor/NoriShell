import type { PasteWarningMode } from "../stores/terminalPreferences";

export const MAX_PASTE_BYTES = 64 * 1_024;
const encoder = new TextEncoder();

export interface PasteAnalysis {
  text: string;
  preview: string;
  lineCount: number;
  hasControls: boolean;
  hasTrailingNewline: boolean;
  tooLarge: boolean;
  requiresConfirmation: boolean;
}

export function analyzeTerminalPaste(text: string, mode: PasteWarningMode): PasteAnalysis {
  const lineCount = text.split(/\r\n|\r|\n/).length;
  // Render ESC, C0/C1, and bidirectional controls explicitly so previews cannot hide bytes about to be sent.
  // eslint-disable-next-line no-control-regex -- Paste-risk checks must identify terminal control bytes.
  const controls = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/g;
  const hasControls = controls.test(text);
  controls.lastIndex = 0;
  return {
    text,
    preview: text.replace(controls, (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`)
      .replace(/\r\n/g, "\n").replace(/\r/g, "\n"),
    lineCount,
    hasControls,
    hasTrailingNewline: /[\r\n]$/.test(text),
    tooLarge: encoder.encode(text).length > MAX_PASTE_BYTES,
    requiresConfirmation: mode === "always" || (mode === "multiline" && (lineCount > 1 || hasControls)),
  };
}

/** Keep xterm's newline and bracketed-paste semantics; never append Enter. */
export function encodeTerminalPaste(text: string, bracketed: boolean) {
  const normalized = text.replace(/\r?\n/g, "\r");
  // An embedded closing marker cannot terminate bracketed-paste mode early.
  if (bracketed && normalized.includes("\u001b[201~")) return null;
  return bracketed ? `\u001b[200~${normalized}\u001b[201~` : normalized;
}
