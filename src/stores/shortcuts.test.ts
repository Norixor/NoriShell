import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

import { SHORTCUT_PREFERENCES_KEY, useShortcutsStore } from "./shortcuts";

describe("shortcuts store", () => {
  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it("persists each platform independently and rejects a conflict without replacing either action", () => {
    const store = useShortcutsStore();
    expect(store.setBinding("macos", "terminal.search", "Meta+Shift+KeyG")).toEqual({ ok: true });
    expect(store.macosBindings["terminal.search"]).toBe("Meta+Shift+KeyG");
    expect(store.windowsBindings["terminal.search"]).toBe("Ctrl+KeyF");
    expect(store.setBinding("macos", "terminal.copy", "Meta+Shift+KeyG")).toMatchObject({
      ok: false,
      reason: "conflict",
      conflicts: ["terminal.search"],
    });
    expect(store.macosBindings["terminal.copy"]).toBe("Meta+Shift+KeyC");

    setActivePinia(createPinia());
    expect(useShortcutsStore().macosBindings["terminal.search"]).toBe("Meta+Shift+KeyG");
  });

  it("keeps the previous configuration when persistence rejects an update or import", () => {
    const store = useShortcutsStore();
    const previous = store.macosBindings["terminal.search"];
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new DOMException("Quota", "QuotaExceededError"); });

    expect(store.setBinding("macos", "terminal.search", "Meta+Shift+KeyG")).toEqual({
      ok: false,
      reason: "storage-error",
    });
    expect(store.macosBindings["terminal.search"]).toBe(previous);
    expect(store.importProfile(store.exportProfile())).toEqual({ ok: false, reason: "storage-error" });
    expect(localStorage.getItem(SHORTCUT_PREFERENCES_KEY)).toBeNull();
  });
});
