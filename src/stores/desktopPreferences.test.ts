import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useDesktopPreferencesStore } from "./desktopPreferences";

const api = vi.hoisted(() => ({ getDesktopPreferences: vi.fn(), replaceDesktopPreferences: vi.fn() }));
vi.mock("../core-api/desktop-preferences", () => api);

const preferences = {
  windowCloseBehavior: "hide" as const,
  trayShowStatus: true,
  trayRecentLimit: 5,
  trayShowHostNames: true,
  notificationBackgroundOnly: true,
  notificationFailureOnly: false,
  notifyTransferCompleted: false,
  notifyTransferFailed: false,
  notifyDisconnected: false,
};

describe("Core desktop preferences projection", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    setActivePinia(createPinia());
  });

  it("does not invent defaults when Core is unavailable", async () => {
    api.getDesktopPreferences.mockRejectedValue(new Error("offline"));
    const store = useDesktopPreferencesStore();
    expect(await store.refresh()).toBe(false);
    expect(store.snapshot).toBeNull();
    expect(await store.replace(preferences)).toBe(false);
    expect(api.replaceDesktopPreferences).not.toHaveBeenCalled();
  });

  it("uses the exact revision and retains the previous projection on conflict", async () => {
    api.getDesktopPreferences.mockResolvedValue({ revision: "9007199254740993", preferences });
    api.replaceDesktopPreferences.mockRejectedValue(new Error("conflict"));
    const store = useDesktopPreferencesStore();
    await store.refresh();
    const next = { ...preferences, trayRecentLimit: 2 };
    expect(await store.replace(next)).toBe(false);
    expect(api.replaceDesktopPreferences).toHaveBeenCalledWith(next, "9007199254740993");
    expect(api.replaceDesktopPreferences).toHaveBeenCalledTimes(1);
    expect(store.snapshot?.preferences.trayRecentLimit).toBe(5);
    expect(store.error).toBe("saveFailed");
  });

  it("prevents a refresh or second mutation from overwriting a pending save", async () => {
    api.getDesktopPreferences.mockResolvedValue({ revision: "1", preferences });
    let complete!: (value: unknown) => void;
    api.replaceDesktopPreferences.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    const store = useDesktopPreferencesStore();
    await store.refresh();
    const next = { ...preferences, trayShowHostNames: false };
    const pending = store.replace(next);
    expect(await store.refresh()).toBe(false);
    expect(await store.replace(preferences)).toBe(false);
    complete({ revision: "2", preferences: next });
    expect(await pending).toBe(true);
    expect(store.snapshot?.revision).toBe("2");
    expect(store.snapshot?.preferences.trayShowHostNames).toBe(false);
  });
});
