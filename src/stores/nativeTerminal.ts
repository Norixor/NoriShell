import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";
import { isTauri } from "@tauri-apps/api/core";

import { getNativeTerminalSettings, getNativeTerminalSnapshot } from "../core-api/native-terminal";
import type { NativeTerminalCommandCompletion, NativeTerminalSessionScope, NativeTerminalSessionStatus, NativeTerminalSettingsSnapshot } from "../core-api/generated/core-api";

export function sameNativeSession(left: NativeTerminalSessionScope, right: NativeTerminalSessionScope) {
  return left.kind === right.kind && left.sessionId === right.sessionId && left.generation === right.generation && left.paneId === right.paneId
    && (left.kind !== "ssh" || right.kind !== "ssh" || left.channelId === right.channelId)
    && (left.kind !== "local" || right.kind !== "local" || left.ptyId === right.ptyId);
}

export const useNativeTerminalStore = defineStore("nativeTerminal", () => {
  const settingsSnapshot = shallowRef<NativeTerminalSettingsSnapshot | null>(null);
  const sessions = shallowRef<NativeTerminalSessionStatus[]>([]);
  const available = ref(false);
  const unread = ref<Record<string, NativeTerminalCommandCompletion[]>>({});
  const visiblePaneId = ref<string | null>(null);
  const appFocused = ref(false);
  const historyEpoch = ref(0);
  let completionCursor: string | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = true;
  let inFlight: Promise<void> | null = null;
  let lifecycle = 0;

  function applySettings(value: NativeTerminalSettingsSnapshot) {
    const previous = settingsSnapshot.value;
    if (!previous || previous.settingsRevision !== value.settingsRevision || previous.historyAvailable !== value.historyAvailable
      || previous.historyPersistenceFailed !== value.historyPersistenceFailed) historyEpoch.value++;
    settingsSnapshot.value = value;
  }
  function acknowledgeVisible() {
    if (!appFocused.value || !visiblePaneId.value || !unread.value[visiblePaneId.value]) return;
    const next = { ...unread.value };
    delete next[visiblePaneId.value];
    unread.value = next;
  }
  function setVisiblePane(paneId: string | null) { visiblePaneId.value = paneId; acknowledgeVisible(); }
  function setAppFocused(focused: boolean) { appFocused.value = focused; acknowledgeVisible(); }
  function sessionStatus(scope: NativeTerminalSessionScope | null) {
    if (!scope || !available.value) return null;
    return sessions.value.find((status) => sameNativeSession(status.session, scope)) ?? null;
  }
  function acceptCompletions(items: NativeTerminalCommandCompletion[]) {
    const next = { ...unread.value };
    const config = settingsSnapshot.value?.settings;
    if (!config?.notificationsEnabled) return;
    for (const item of items) {
      if (item.elapsedMillis < config.notificationThresholdSeconds * 1_000) continue;
      const paneId = item.session.paneId;
      if (appFocused.value && visiblePaneId.value === paneId) continue;
      const existing = next[paneId] ?? [];
      if (!existing.some((prior) => prior.eventId === item.eventId)) next[paneId] = [...existing, item].slice(-20);
    }
    unread.value = next;
  }
  async function refresh() {
    if (!isTauri()) return;
    if (inFlight) return inFlight;
    const generation = lifecycle;
    const request = (async () => {
      try {
        const [settings, snapshot] = await Promise.all([getNativeTerminalSettings(), getNativeTerminalSnapshot(completionCursor)]);
        if (generation !== lifecycle) return;
        applySettings(settings);
        sessions.value = snapshot.sessions;
        unread.value = Object.fromEntries(Object.entries(unread.value).flatMap(([paneId, items]) => {
          const live = items.filter((item) => snapshot.sessions.some((status) => sameNativeSession(status.session, item.session)));
          return live.length ? [[paneId, live]] : [];
        }));
        // The first read establishes only the cursor; a renderer reload cannot replay old notifications.
        if (completionCursor !== null) acceptCompletions(snapshot.completions.filter((item) => snapshot.sessions.some((status) => sameNativeSession(status.session, item.session))));
        completionCursor = snapshot.completionCursor;
        available.value = true;
      } catch {
        if (generation !== lifecycle) return;
        available.value = false;
        historyEpoch.value++;
      }
    })().finally(() => { if (inFlight === request) inFlight = null; });
    inFlight = request;
    return request;
  }
  function start() {
    if (!stopped || !isTauri()) return;
    stopped = false;
    const generation = lifecycle;
    const tick = async () => {
      await refresh();
      if (!stopped && generation === lifecycle) timer = setTimeout(tick, 1_000);
    };
    void tick();
  }
  function stop() {
    stopped = true;
    lifecycle++;
    inFlight = null;
    completionCursor = null;
    unread.value = {};
    clearTimeout(timer);
    timer = undefined;
    sessions.value = [];
    available.value = false;
    historyEpoch.value++;
  }
  return { settingsSnapshot, sessions, available, unread, visiblePaneId, appFocused, historyEpoch, applySettings, sessionStatus, setVisiblePane, setAppFocused, refresh, start, stop };
});
