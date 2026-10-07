import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createMobileImeInput, textareaEdit, type MobileImeInputController } from "./mobile-ime-input";

describe("textareaEdit", () => {
  it("derives deletions and insertions from the common prefix by code point", () => {
    expect(textareaEdit("", "a")).toEqual({ deleted: 0, inserted: "a" });
    expect(textareaEdit("helo ", "hello ")).toEqual({ deleted: 2, inserted: "lo " });
    expect(textareaEdit("ab😀", "ab")).toEqual({ deleted: 1, inserted: "" });
  });
});

describe("createMobileImeInput", () => {
  let root: HTMLDivElement;
  let textarea: HTMLTextAreaElement;
  let delivered: string[];
  let previews: Array<string | null>;
  let pasted: string[];
  let controller: MobileImeInputController;
  let xtermSawInput: ReturnType<typeof vi.fn<(event: Event) => void>>;

  beforeEach(() => {
    vi.useFakeTimers();
    root = document.createElement("div");
    textarea = document.createElement("textarea");
    root.append(textarea);
    document.body.append(root);
    delivered = [];
    previews = [];
    pasted = [];
    xtermSawInput = vi.fn<(event: Event) => void>();
    // Stand-in for xterm's own at-target textarea listeners.
    for (const type of ["input", "compositionstart", "compositionupdate", "compositionend"]) {
      textarea.addEventListener(type, xtermSawInput, true);
    }
    controller = createMobileImeInput({
      eventRoot: root,
      textarea: () => textarea,
      deliver: (data) => delivered.push(data),
      paste: (text) => pasted.push(text),
      backspace: () => "\x7f",
      preview: (text) => previews.push(text),
    });
  });

  afterEach(() => {
    controller.dispose();
    root.remove();
    vi.useRealTimers();
  });

  function typeText(data: string, inputType = "insertText") {
    textarea.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, data, inputType }));
    textarea.value += data;
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data, inputType }));
  }

  function compose(updates: string[], commit: string) {
    const base = textarea.value;
    textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    for (const update of updates) {
      textarea.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: update }));
      textarea.value = base + update;
      textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: update, inputType: "insertCompositionText", isComposing: true }));
    }
    textarea.value = base + commit;
    textarea.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: commit }));
  }

  it("delivers each soft-keyboard character exactly once and keeps xterm out of the text path", () => {
    for (const character of "echo hello") typeText(character);
    expect(delivered.join("")).toBe("echo hello");
    expect(xtermSawInput).not.toHaveBeenCalled();
    vi.runAllTimers();
    expect(textarea.value).toBe("");
    typeText("x");
    expect(delivered.join("")).toBe("echo hellox");
  });

  it("is not confused when xterm clears the textarea itself between events", () => {
    typeText("a");
    textarea.value = "";
    typeText("b");
    expect(delivered).toEqual(["a", "b"]);
  });

  it("commits a pinyin composition once and only previews it before", () => {
    compose(["n", "ni", "ni h", "ni ha", "ni hao"], "你好");
    expect(delivered).toEqual(["你好"]);
    expect(previews.at(0)).toBe("");
    expect(previews.slice(1, -1).filter((preview) => preview)).toEqual(expect.arrayContaining(["n", "ni", "ni h", "ni ha", "ni hao"]));
    expect(previews.at(-1)).toBeNull();
    vi.runAllTimers();
    expect(textarea.value).toBe("");
    expect(controller.isComposing()).toBe(false);
  });

  it("handles predictive replacements as backspaces followed by the new text", () => {
    compose(["helo"], "helo");
    expect(delivered).toEqual(["helo"]);
    // Before the textarea is emptied, the IME recomposes the last word and replaces it.
    textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "helo" }));
    textarea.value = "hello ";
    textarea.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "hello " }));
    expect(delivered).toEqual(["helo", "\x7flo "]);
  });

  it("maps deletions and line breaks to terminal keys", () => {
    typeText("ab");
    textarea.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, inputType: "deleteContentBackward" }));
    textarea.value = "a";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "deleteContentBackward" }));
    typeText("\n", "insertLineBreak");
    expect(delivered).toEqual(["ab", "\x7f", "\r"]);
  });

  it("does not stay stuck composing when compositionend never arrives", () => {
    textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    textarea.value = "wor";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: "wor", inputType: "insertCompositionText", isComposing: true }));
    expect(controller.isComposing()).toBe(true);
    textarea.value = "word";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true, data: "d", inputType: "insertText" }));
    expect(controller.isComposing()).toBe(false);
    expect(delivered).toEqual(["word"]);
  });

  it("commits pending composition once on blur", () => {
    textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    textarea.value = "ok";
    textarea.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    textarea.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "ok" }));
    expect(delivered).toEqual(["ok"]);
    expect(textarea.value).toBe("");
  });

  it("routes a multi-line keyboard clipboard chip commit to the paste guard instead of typing it", () => {
    // Gboard's clipboard suggestion commits the whole clip as one insertText.
    typeText("echo one\necho two\n");
    expect(delivered).toEqual([]);
    expect(pasted).toEqual(["echo one\necho two\n"]);
  });

  it("routes multi-line or control-character IME commits and replacement text to the paste guard", () => {
    compose(["a"], "line1\nline2");
    typeText("tab\there", "insertReplacementText");
    expect(delivered).toEqual([]);
    expect(pasted).toEqual(["line1\nline2", "tab\there"]);
  });

  it("cancels insertFromPaste and insertFromDrop and hands their data to the paste guard", () => {
    for (const inputType of ["insertFromPaste", "insertFromDrop"]) {
      const event = new InputEvent("beforeinput", { bubbles: true, cancelable: true, inputType, data: "one\ntwo" });
      textarea.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    }
    expect(pasted).toEqual(["one\ntwo", "one\ntwo"]);
    expect(delivered).toEqual([]);
  });

  it("treats an uncancelable insertFromPaste as a paste even when it is a single line", () => {
    textarea.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, cancelable: false, inputType: "insertFromPaste", data: "ls" }));
    textarea.value = "ls";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste", data: "ls" }));
    expect(pasted).toEqual(["ls"]);
    expect(delivered).toEqual([]);
    expect(textarea.value).toBe("");
  });

  it("keeps single-line typing and the IME Enter key unchanged", () => {
    typeText("ls -la");
    typeText("\n", "insertLineBreak");
    expect(delivered).toEqual(["ls -la", "\r"]);
    expect(pasted).toEqual([]);
  });
});
