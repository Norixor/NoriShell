import { describe, expect, it } from "vitest";
import { DEFAULT_HIGHLIGHT_RULES, matchHighlightLines, validateHighlightConfiguration, validateHighlightRule } from "./highlighting";
import { highlightCellRanges } from "./xtermHighlighting";

describe("keyword highlighting", () => {
  it("recognizes common log tokens without matching parts of identifiers", () => {
    const lines = ["[critical] PANIC failed failure", "warning WARN", "OK successful succeeded PASS passed", "[INFO] notice"];
    const matches = matchHighlightLines(lines, DEFAULT_HIGHLIGHT_RULES);
    expect(matches.map(match => [match.ruleId, lines[match.line]!.slice(match.start, match.end)])).toEqual([
      ["error", "critical"], ["error", "PANIC"], ["error", "failed"], ["error", "failure"],
      ["warning", "warning"], ["warning", "WARN"],
      ["success", "OK"], ["success", "successful"], ["success", "succeeded"], ["success", "PASS"], ["success", "passed"],
      ["info", "INFO"], ["info", "notice"],
    ]);
    expect(matchHighlightLines(["book password bypass information errorCount ERROR_CODE DEBUG TRACE 192.168.1.1"], DEFAULT_HIGHLIGHT_RULES)).toEqual([]);
    const debug = DEFAULT_HIGHLIGHT_RULES.find(rule => rule.id === "debug")!;
    expect(matchHighlightLines(["DEBUG TRACE"], [{ ...debug, enabled: true }])).toHaveLength(2);
  });
  it("matches literal metacharacters, case and regex independently", () => {
    const literal = { ...DEFAULT_HIGHLIGHT_RULES[0]!, id: "literal", mode: "literal" as const, pattern: "a.b[0]", caseSensitive: true };
    const matches = matchHighlightLines(["error A.B[0] a.b[0]", "WARNING"], [...DEFAULT_HIGHLIGHT_RULES, literal]);
    expect(matches).toContainEqual({ line: 0, start: 0, end: 5, ruleId: "error" });
    expect(matches.filter((match) => match.ruleId === "literal")).toEqual([{ line: 0, start: 13, end: 19, ruleId: "literal" }]);
    expect(matchHighlightLines(["ERROR"], [{ ...literal, enabled: false }])).toEqual([]);
  });
  it("rejects malformed rules and caps matching volume", () => {
    expect(validateHighlightRule({ ...DEFAULT_HIGHLIGHT_RULES[0], pattern: "(" })).toBe(false);
    expect(validateHighlightRule({ ...DEFAULT_HIGHLIGHT_RULES[0], foreground: "url(example)" })).toBe(false);
    expect(validateHighlightConfiguration({ enabled: true, rules: [DEFAULT_HIGHLIGHT_RULES[0], DEFAULT_HIGHLIGHT_RULES[0]] })).toBe(false);
    expect(matchHighlightLines(["x".repeat(4_096)], [{ ...DEFAULT_HIGHLIGHT_RULES[0]!, pattern: "x" }])).toHaveLength(1_000);
    expect(matchHighlightLines(["test"], [{ ...DEFAULT_HIGHLIGHT_RULES[0]!, pattern: "(?:)" }])).toEqual([]);
  });
  it("maps CJK and surrogate offsets back to whole cells across soft wraps", () => {
    const positions = [
      { row: 3, column: 0, width: 2 },
      { row: 3, column: 2, width: 2 }, { row: 3, column: 2, width: 2 },
      { row: 4, column: 0, width: 1 },
    ];
    expect(highlightCellRanges({ text: "中😀E", positions }, 0, 4)).toEqual([
      { row: 3, column: 0, width: 4 }, { row: 4, column: 0, width: 1 },
    ]);
  });
});
