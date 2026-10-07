import { beforeEach, describe, expect, it } from "vitest";

import {
  applyStickyModifiers, clearStickyModifiers, controlCharacter, extraKeySequence, stickyAlt, stickyCtrl,
  toggleStickyAlt, toggleStickyCtrl,
} from "./extra-keys";

beforeEach(() => clearStickyModifiers());

describe("extra keys", () => {
  it("encodes plain helper keys and honors DECCKM for cursor keys", () => {
    expect(extraKeySequence("escape", false)).toBe("\x1b");
    expect(extraKeySequence("tab", false)).toBe("\t");
    expect(extraKeySequence("arrowUp", false)).toBe("\x1b[A");
    expect(extraKeySequence("arrowUp", true)).toBe("\x1bOA");
    expect(extraKeySequence("home", true)).toBe("\x1bOH");
    expect(extraKeySequence("pageDown", false)).toBe("\x1b[6~");
    expect(extraKeySequence("pipe", false)).toBe("|");
    expect(extraKeySequence("dash", false)).toBe("-");
  });

  it("applies and consumes sticky Ctrl and Alt on helper keys", () => {
    toggleStickyCtrl();
    expect(extraKeySequence("arrowLeft", true)).toBe("\x1b[1;5D");
    expect(stickyCtrl.value).toBe(false);
    toggleStickyAlt();
    expect(extraKeySequence("pageUp", false)).toBe("\x1b[5;3~");
    expect(stickyAlt.value).toBe(false);
    toggleStickyCtrl();
    toggleStickyAlt();
    expect(extraKeySequence("arrowUp", false)).toBe("\x1b[1;7A");
  });

  it("turns the next typed character into its control or meta form", () => {
    expect(applyStickyModifiers("c")).toBe("c");
    toggleStickyCtrl();
    expect(applyStickyModifiers("c")).toBe("\x03");
    expect(applyStickyModifiers("c")).toBe("c");
    toggleStickyAlt();
    expect(applyStickyModifiers("x")).toBe("\x1bx");
    toggleStickyCtrl();
    toggleStickyAlt();
    expect(applyStickyModifiers("a")).toBe("\x1b\x01");
  });

  it("passes sequences and multi-character commits through but still consumes the modifier", () => {
    toggleStickyCtrl();
    expect(applyStickyModifiers("\x1b[A")).toBe("\x1b[A");
    expect(stickyCtrl.value).toBe(false);
    toggleStickyCtrl();
    expect(applyStickyModifiers("你好")).toBe("你好");
    expect(stickyCtrl.value).toBe(false);
  });

  it("maps control characters like a hardware keyboard", () => {
    expect(controlCharacter("c")).toBe("\x03");
    expect(controlCharacter("[")).toBe("\x1b");
    expect(controlCharacter("?")).toBe("\x7f");
    expect(controlCharacter(" ")).toBe("\x00");
    expect(controlCharacter("%")).toBeNull();
  });
});
