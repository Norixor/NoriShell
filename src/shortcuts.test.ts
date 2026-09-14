import { describe, expect, it } from "vitest";

import {
  SHORTCUT_COMMANDS,
  createDefaultShortcutBindings,
  findShortcutConflicts,
  formatShortcutBinding,
  isShortcutExecutionAllowed,
  matchShortcut,
  parseShortcutProfile,
  resolveShortcut,
  serializeShortcutProfile,
  shortcutFromKeyboardEvent,
  shouldConsumeShortcut,
  validateShortcutBinding,
} from "./shortcuts";

describe("shortcut normalization and resolution", () => {
  it("normalizes aliases into a canonical platform binding", () => {
    expect(validateShortcutBinding("cmd + shift + k", "macos", "app")).toEqual({
      binding: "Meta+Shift+KeyK",
      error: null,
    });
    expect(formatShortcutBinding("Meta+Shift+KeyK", "macos")).toBe("⌘⇧K");
    expect(validateShortcutBinding("Ctrl+Alt+Delete", "windows", "app").error).toBe("system-reserved");
    for (const binding of ["Meta+KeyC", "Meta+KeyV", "Meta+KeyX", "Meta+KeyZ", "Meta+KeyA", "Meta+Shift+KeyZ"]) {
      expect(validateShortcutBinding(binding, "macos", "app").error).toBe("system-reserved");
    }
  });

  it("rejects wrong primary modifiers and records supported key events", () => {
    expect(validateShortcutBinding("Alt+KeyO", "windows", "app").error).toBe("missing-modifier");
    const event = new KeyboardEvent("keydown", { code: "KeyP", ctrlKey: true, shiftKey: true });
    expect(shortcutFromKeyboardEvent(event, "windows", "terminal")).toEqual({
      binding: "Ctrl+Shift+KeyP",
      error: null,
    });
  });

  it("does not resolve behind editable fields, dialogs, or an inactive terminal", () => {
    const bindings = createDefaultShortcutBindings("macos");
    const search = new KeyboardEvent("keydown", { code: "KeyF", metaKey: true });
    expect(resolveShortcut(search, "macos", bindings, { terminalActive: true })?.id).toBe("terminal.search");
    expect(resolveShortcut(search, "macos", bindings, { terminalActive: true, editableTarget: true })).toBeNull();
    expect(resolveShortcut(search, "macos", bindings, { terminalActive: true, modalOpen: true })).toBeNull();
    expect(resolveShortcut(search, "macos", bindings, { terminalActive: false })).toBeNull();
  });

  it("keeps xterm helper input eligible while ordinary fields block execution", () => {
    const bindings = createDefaultShortcutBindings("macos");
    const xterm = document.createElement("div");
    xterm.className = "xterm";
    const helper = document.createElement("textarea");
    helper.className = "xterm-helper-textarea";
    xterm.append(helper);
    document.body.append(xterm);
    let resolved: ReturnType<typeof resolveShortcut> = null;
    helper.addEventListener("keydown", (event) => {
      resolved = resolveShortcut(event, "macos", bindings, { terminalActive: true });
    });
    helper.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyF", metaKey: true, bubbles: true }));
    expect(resolved).toMatchObject({ id: "terminal.search" });

    const field = document.createElement("input");
    document.body.append(field);
    field.addEventListener("keydown", (event) => {
      resolved = resolveShortcut(event, "macos", bindings, { terminalActive: true });
    });
    field.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyF", metaKey: true, bubbles: true }));
    expect(resolved).toBeNull();
    xterm.remove();
    field.remove();
  });

  it("separates matching, consuming, and execution so blocked reserved keys do not reach a terminal", () => {
    const bindings = createDefaultShortcutBindings("windows");
    const repeated = new KeyboardEvent("keydown", { code: "Digit1", ctrlKey: true, repeat: true });
    const command = matchShortcut(repeated, "windows", bindings);
    expect(command?.id).toBe("workspace.tab.1");
    expect(isShortcutExecutionAllowed(command!, repeated, { terminalActive: true, modalOpen: true })).toBe(false);
    expect(shouldConsumeShortcut(command, { modalOpen: true })).toBe(true);
    expect(shouldConsumeShortcut(command, { shortcutRecording: true })).toBe(false);
  });

  it("finds conflicts without replacing the prior binding", () => {
    const bindings = createDefaultShortcutBindings("windows");
    expect(findShortcutConflicts(bindings, "workspace.tab.2", "Ctrl+Digit1").map((item) => item.id))
      .toEqual(["workspace.tab.1"]);
    expect(bindings["workspace.tab.1"]).toBe("Ctrl+Digit1");
  });

  it("only accepts a bounded, complete, conflict-free versioned profile", () => {
    const profile = {
      version: 1 as const,
      bindings: {
        macos: createDefaultShortcutBindings("macos"),
        windows: createDefaultShortcutBindings("windows"),
      },
    };
    expect(parseShortcutProfile(serializeShortcutProfile(profile))).toMatchObject({ ok: true });
    expect(parseShortcutProfile("x".repeat(32 * 1024 + 1))).toEqual({ ok: false, error: "too-large" });

    const conflicted = structuredClone(profile);
    conflicted.bindings.macos["workspace.tab.2"] = conflicted.bindings.macos["workspace.tab.1"];
    expect(parseShortcutProfile(JSON.stringify(conflicted)).ok).toBe(false);
    expect(SHORTCUT_COMMANDS).toHaveLength(36);
    expect(profile.bindings.macos["workspace.new-local"]).toBeNull();
    expect(profile.bindings.windows["terminal.reconnect"]).toBe("Ctrl+Shift+KeyR");
  });
});
