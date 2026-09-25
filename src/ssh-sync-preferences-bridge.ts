import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { createPreferenceAdapters } from "./preference-adapters";
import {
  exportFullSyncPreferences, PREFERENCE_GROUP_IDS, preferenceValuesEqual,
  validateFullSyncPreferences, type PreferenceGroupId, type PreferenceGroupResult,
  type PreferenceTransferFile,
} from "./preferences-transfer";

type ApplyResult = Exclude<PreferenceGroupResult, "pending" | "applying">;
export type PendingSshSyncPreferences = {
  id: string;
  ready: boolean;
  expected: PreferenceTransferFile;
  desired: PreferenceTransferFile;
  results: Partial<Record<PreferenceGroupId, ApplyResult>>;
};
const COLLECT_EVENT = "norishell:ssh-sync-preferences:collect";
const PENDING_EVENT = "norishell:ssh-sync-preferences:pending";
let applying: Promise<void> | null = null;
export const SSH_SYNC_PREFERENCES_CHANGED = "norishell:ssh-sync-preferences:changed";
const changed = () => window.dispatchEvent(new Event(SSH_SYNC_PREFERENCES_CHANGED));

async function applyPending(): Promise<void> {
  if (applying) return applying;
  const task = (async () => {
    const adapters = createPreferenceAdapters();
    const pending = await invoke<PendingSshSyncPreferences | null>("ssh_sync_preferences_pending_get");
    if (!pending) return;
    if (!pending.ready) return;
    validateFullSyncPreferences(pending.expected, adapters);
    validateFullSyncPreferences(pending.desired, adapters);
    for (const id of PREFERENCE_GROUP_IDS) {
      if (pending.results[id]) continue;
      const adapter = adapters.find((item) => item.id === id);
      if (!adapter) throw new Error("missingPreferenceAdapter");
      const result: ApplyResult = await (async () => {
        try {
          const expected = pending.expected.groups[id];
          const desired = pending.desired.groups[id];
          const current = await adapter.read();
          if (!adapter.validate(current) || !adapter.validate(expected) || !adapter.validate(desired)) {
            return "failed";
          } else if (preferenceValuesEqual(current, desired)) {
            return "unchanged";
          } else if (!preferenceValuesEqual(current, expected)) {
            return "conflict";
          } else {
            return await adapter.apply(desired, current) ? "applied" : "failed";
          }
        } catch { return "failed"; }
      })();
      // Record each attempt before moving to the next group, so a crash cannot replay an
      // attempted write without a fresh comparison of its current value.
      await invoke("ssh_sync_preferences_apply_ack", { request: { id: pending.id, results: { [id]: result } } });
    }
  })();
  applying = task;
  try { await task; } finally { if (applying === task) applying = null; changed(); }
}

/** Register after Pinia stores exist. Core collection fails closed if this bridge is unavailable. */
export async function startSshSyncPreferencesBridge(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const stopCollect = await listen<{ requestId: string }>(COLLECT_EVENT, async ({ payload }) => {
    try {
      const transfer = await exportFullSyncPreferences(createPreferenceAdapters());
      await invoke("ssh_sync_preferences_publish", { request: { requestId: payload.requestId, transfer } });
    } catch { /* Core times out and rejects the sync; no partial snapshot is published. */ }
  });
  const stopPending = await listen(PENDING_EVENT, () => { changed(); void applyPending().catch(() => {}); });
  void applyPending().catch(() => {});
  return () => { stopCollect(); stopPending(); };
}

/** Invoke only from an explicit preferences review action after showing the unresolved groups. */
export async function resolvePendingSshSyncPreferences(resolution: "retry" | "useRemote" | "keepLocal"): Promise<void> {
  if (applying) await applying.catch(() => {});
  const pending = await invoke<PendingSshSyncPreferences | null>("ssh_sync_preferences_pending_get");
  if (!pending) return;
  const current = resolution === "useRemote" ? await exportFullSyncPreferences(createPreferenceAdapters()) : null;
  await invoke("ssh_sync_preferences_retry_pending", { request: { id: pending.id, resolution, current } });
  if (resolution !== "keepLocal") await applyPending();
  changed();
}

export async function getPendingSshSyncPreferences(): Promise<PendingSshSyncPreferences | null> {
  if (!isTauri()) return null;
  return invoke<PendingSshSyncPreferences | null>("ssh_sync_preferences_pending_get");
}
