import { describe, expect, it } from "vitest";
import { historySuggestionSuffix, TerminalDraftTracker } from "./draft";

describe("shell prompt draft", () => {
  it("tracks only a fresh prompt and append-only edits", () => {
    const tracker = new TerminalDraftTracker();
    expect(tracker.input("secret")).toBeNull();
    tracker.prompt();
    expect(tracker.input("git sta")).toBe("git sta");
    expect(tracker.input("\u007f")).toBe("git st");
    expect(tracker.input("\u001b[D")).toBeNull();
    expect(tracker.input("anything")).toBeNull();
    tracker.prompt();
    expect(tracker.input("echo 中文")).toBe("echo 中文");
    expect(tracker.input("\u007f")).toBeNull();
  });
  it("discards line editing after Enter, completion and control input", () => {
    for (const input of ["\r", "\t", "\u0003", "\u001b[A", "\u001b[200~text\u001b[201~"]) {
      const tracker = new TerminalDraftTracker();
      tracker.prompt();
      tracker.input("prefix");
      expect(tracker.input(input)).toBeNull();
    }
  });
  it("only inserts a matching single-line suffix without adding Enter", () => {
    expect(historySuggestionSuffix("git status", "git st")).toBe("atus");
    expect(historySuggestionSuffix("git status", "rm ")).toBeNull();
    expect(historySuggestionSuffix("git status", null)).toBeNull();
    expect(historySuggestionSuffix("one\ntwo", "")).toBeNull();
    expect(historySuggestionSuffix("echo\u001b[2J", "")).toBeNull();
  });
});
