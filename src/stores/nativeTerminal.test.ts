import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import type { NativeTerminalCommandCompletion, NativeTerminalSessionStatus, NativeTerminalSettingsSnapshot, NativeTerminalSnapshot } from "../core-api/generated/core-api";
import { useNativeTerminalStore } from "./nativeTerminal";

const api = vi.hoisted(() => ({ settings: vi.fn(), snapshot: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("../core-api/native-terminal", () => ({ getNativeTerminalSettings: api.settings, getNativeTerminalSnapshot: api.snapshot }));
const scope = { kind: "local" as const, sessionId: "session-a", generation: "1", ptyId: "pty-a", paneId: "pane-a" };
const settings: NativeTerminalSettingsSnapshot = { settingsRevision: "1", historyAvailable: true, historyPersistenceFailed: false, settings: { historyEnabled: true, persistEncrypted: false, historyMaxEntries: 500, historyRetentionDays: 30, historyPaused: false, notificationsEnabled: true, notificationThresholdSeconds: 60 } };
const status: NativeTerminalSessionStatus = { session: scope, shellKind: "zsh", captureState: "ready", failureCode: null, activity: "prompt", capturesCommand: true, historyPaused: false, promptObserved: true, promptSequence: "1", promptInputSequence: "1", promptInputEpoch: "1", completionCursor: "1" };
function completion(id: string, elapsedMillis = 61_000): NativeTerminalCommandCompletion { return { eventId: id, cursor: id, session: scope, elapsedMillis, exitCode: 0, completedAtUnixMs: 1 }; }
function snapshot(cursor: string, completions: NativeTerminalCommandCompletion[] = [], sessions = [status]): NativeTerminalSnapshot { return { schemaVersion: 2, snapshotRevision: cursor, historyPaused: false, historyPersistenceFailed: false, completionCursor: cursor, sessions, completions }; }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((r) => { resolve = r; }); return { promise, resolve }; }

beforeEach(() => { setActivePinia(createPinia()); vi.useFakeTimers(); api.settings.mockReset().mockResolvedValue(settings); api.snapshot.mockReset(); });
afterEach(() => { useNativeTerminalStore().stop(); vi.useRealTimers(); });
describe("native terminal projection", () => {
  it("skips initial replay, applies threshold, deduplicates and acknowledges only the visible focused pane", async () => {
    const store = useNativeTerminalStore();
    api.snapshot.mockResolvedValueOnce(snapshot("1", [completion("1")])); await store.refresh();
    expect(store.unread).toEqual({});
    api.snapshot.mockResolvedValueOnce(snapshot("3", [completion("2", 1_000), completion("3"), completion("3")])); await store.refresh();
    expect(store.unread[scope.paneId]).toHaveLength(1);
    store.setVisiblePane(scope.paneId); expect(store.unread[scope.paneId]).toHaveLength(1);
    store.setAppFocused(true); expect(store.unread).toEqual({});
    api.snapshot.mockResolvedValueOnce(snapshot("4", [completion("4")])); await store.refresh(); expect(store.unread).toEqual({});
  });
  it("drops completed-session badges instead of assigning them to a replacement generation", async () => {
    const store = useNativeTerminalStore();
    api.snapshot.mockResolvedValueOnce(snapshot("0")); await store.refresh();
    api.snapshot.mockResolvedValueOnce(snapshot("1", [completion("1")])); await store.refresh();
    expect(store.unread[scope.paneId]).toHaveLength(1);
    api.snapshot.mockResolvedValueOnce(snapshot("2", [completion("2")], [{ ...status, session: { ...scope, generation: "2" } }])); await store.refresh();
    expect(store.unread).toEqual({});
  });
  it("invalidates visible history when Vault availability changes or a request fails", async () => {
    const store = useNativeTerminalStore(); api.snapshot.mockResolvedValue(snapshot("1")); await store.refresh(); const epoch = store.historyEpoch;
    api.settings.mockResolvedValue({ ...settings, historyAvailable: false }); await store.refresh(); expect(store.historyEpoch).toBeGreaterThan(epoch);
    api.snapshot.mockRejectedValue(new Error("offline")); await store.refresh(); expect(store.available).toBe(false); expect(store.sessionStatus(scope)).toBeNull();
  });
  it("ignores an old lifecycle response without clearing a restarted in-flight request or duplicating poll timers", async () => {
    const store = useNativeTerminalStore(); const old = deferred<NativeTerminalSnapshot>(); const fresh = deferred<NativeTerminalSnapshot>();
    api.snapshot.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise).mockResolvedValue(snapshot("3"));
    store.start(); store.stop(); store.start(); old.resolve(snapshot("99")); await vi.advanceTimersByTimeAsync(0);
    expect(store.available).toBe(false); const active = store.refresh(); expect(api.snapshot).toHaveBeenCalledTimes(2);
    fresh.resolve(snapshot("2")); await active; expect(store.available).toBe(true); await vi.advanceTimersByTimeAsync(1_000);
    expect(api.snapshot).toHaveBeenCalledTimes(3);
  });
});
