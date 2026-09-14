/** Tracks only simple appended input after a confirmed prompt. Unknown line edits are abandoned rather than guessing the Shell buffer. */
export class TerminalDraftTracker {
  private text: string | null = null;
  prompt() { this.text = ""; return this.text; }
  invalidate() { this.text = null; return this.text; }
  value() { return this.text; }
  input(value: string) {
    if (this.text === null) return null;
    if (value === "\u007f" || value === "\b") {
      const chars = [...this.text];
      // Do not guess how different Shells delete grapheme clusters or emoji.
      if (/[^\x20-\x7e]/.test(chars.at(-1) ?? "")) return this.invalidate();
      chars.pop();
      this.text = chars.join("");
    } else if (value && !/[\p{Cc}\p{Cf}]/u.test(value) && this.text.length + value.length <= 4_096) {
      this.text += value;
    } else return this.invalidate();
    return this.text;
  }
}

export function historySuggestionSuffix(command: string, draft: string | null): string | null {
  if (draft === null || !command || command.length > 4_096 || /[\p{Cc}\p{Cf}]/u.test(command)) return null;
  if (!command.startsWith(draft) || command === draft) return null;
  return command.slice(draft.length);
}
