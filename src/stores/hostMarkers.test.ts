import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HOST_MARKERS_STORAGE_KEY, useHostMarkersStore } from "./hostMarkers";

describe("local host markers", () => {
  beforeEach(() => { vi.restoreAllMocks(); localStorage.clear(); setActivePinia(createPinia()); });
  it("persists per host and hides without deleting preferences", () => {
    const store = useHostMarkersStore();
    expect(store.set("host-a", { kind: "production", color: "red" })).toBe("saved");
    expect(store.set("host-b", { kind: "custom", label: "  API  ", color: "blue" })).toBe("saved");
    expect(store.setEnabled(false)).toBe("saved");
    expect(store.visibleMarker("host-a")).toBeNull();
    setActivePinia(createPinia());
    const restored = useHostMarkersStore();
    expect(restored.get("host-b")).toEqual({ kind: "custom", label: "API", color: "blue" });
    expect(restored.enabled).toBe(false);
    expect(restored.remove("host-a")).toBe("saved");
    expect(restored.get("host-b")).not.toBeNull();
  });
  it("preserves old state when saving, clearing, or toggling fails", () => {
    const store = useHostMarkersStore();
    store.set("a", { kind: "testing", color: "amber" });
    const storageSpy = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    expect(store.set("a", { kind: "development", color: "green" })).toBe("storage-error");
    expect(store.remove("a")).toBe("storage-error");
    expect(store.setEnabled(false)).toBe("storage-error");
    expect(store.get("a")).toEqual({ kind: "testing", color: "amber" });
    expect(store.enabled).toBe(true);
    storageSpy.mockRestore();
  });
  it("rejects invalid text and safely ignores damaged or unknown versions", () => {
    const store = useHostMarkersStore();
    for (const label of ["", "a".repeat(13), "a\nb", "x\u202e"]) {
      expect(store.set("a", { kind: "custom", label, color: "blue" })).toBe("invalid");
    }
    localStorage.setItem(HOST_MARKERS_STORAGE_KEY, JSON.stringify({ version: 7, enabled: false, markers: {} }));
    setActivePinia(createPinia());
    expect(useHostMarkersStore().enabled).toBe(true);
    localStorage.setItem(HOST_MARKERS_STORAGE_KEY, JSON.stringify({ version: 1, enabled: true, markers: { a: { kind: "custom", label: "ok", color: "url(x)" } } }));
    setActivePinia(createPinia());
    expect(useHostMarkersStore().get("a")).toBeNull();
  });
});
