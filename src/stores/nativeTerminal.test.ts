import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import type { NativeTerminalSettingsSnapshot } from "../core-api/generated/core-api";
import { useNativeTerminalStore } from "./nativeTerminal";

const api = vi.hoisted(() => ({ settings: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("../core-api/native-terminal", () => ({ getNativeTerminalSettings: api.settings }));
const settings: NativeTerminalSettingsSnapshot = { settingsRevision: "1", historyAvailable: true, historyPersistenceFailed: false, settings: { historyEnabled: true, persistEncrypted: true, historyMaxEntries: 500, historyRetentionDays: 30, historyPaused: false, notificationsEnabled: false, notificationThresholdSeconds: 60 } };
beforeEach(() => { setActivePinia(createPinia()); api.settings.mockReset().mockResolvedValue(settings); });

describe("native terminal settings projection", () => {
  it("invalidates history on settings, Vault, and local history mutations", async () => {
    const store = useNativeTerminalStore();
    await store.refresh(); const initial = store.historyEpoch;
    store.historyChanged(); expect(store.historyEpoch).toBeGreaterThan(initial);
    const changed = store.historyEpoch;
    api.settings.mockResolvedValue({ ...settings, historyAvailable: false });
    await store.refresh(); expect(store.historyEpoch).toBeGreaterThan(changed);
  });
  it("does not claim availability when settings cannot be read", async () => {
    const store = useNativeTerminalStore();
    api.settings.mockRejectedValue(new Error("offline"));
    await store.refresh();
    expect(store.available).toBe(false);
  });
});
