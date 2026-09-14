import { describe, expect, it } from "vitest";
import { analyzeTerminalPaste, encodeTerminalPaste } from "./paste";

describe("terminal paste", () => {
  it("warns for CR, LF, CRLF and a trailing newline but preserves the submitted text", () => {
    for (const newline of ["\r", "\n", "\r\n"]) {
      const input = `printf test${newline}`;
      const analysis = analyzeTerminalPaste(input, "multiline");
      expect(analysis).toMatchObject({ text: input, lineCount: 2, hasTrailingNewline: true, requiresConfirmation: true });
    }
    expect(analyzeTerminalPaste("echo test", "multiline").requiresConfirmation).toBe(false);
    expect(analyzeTerminalPaste("echo test", "always").requiresConfirmation).toBe(true);
    expect(analyzeTerminalPaste("one\ntwo", "never").requiresConfirmation).toBe(false);
  });
  it("exposes control characters and rejects a bracketed-paste escape", () => {
    const analysis = analyzeTerminalPaste("ab\u202ecd\u001b[2J", "multiline");
    expect(analysis.hasControls).toBe(true);
    expect(analysis.preview).toBe("ab\\u202ecd\\u001b[2J");
    expect(encodeTerminalPaste("echo\nnext", true)).toBe("\u001b[200~echo\rnext\u001b[201~");
    expect(encodeTerminalPaste("echo\u001b[201~bad", true)).toBeNull();
    expect(encodeTerminalPaste("echo", false)).toBe("echo");
  });
  it("bounds UTF-8 bytes, not just JavaScript character count", () => {
    expect(analyzeTerminalPaste("中".repeat(22_000), "never").tooLarge).toBe(true);
    expect(analyzeTerminalPaste("x".repeat(65_536), "never").tooLarge).toBe(false);
  });
});
