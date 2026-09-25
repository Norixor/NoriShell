import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";
import { isTauri } from "@tauri-apps/api/core";

import { getNativeTerminalSettings } from "../core-api/native-terminal";
import type { NativeTerminalSettingsSnapshot } from "../core-api/generated/core-api";

export const useNativeTerminalStore = defineStore("nativeTerminal", () => {
  const settingsSnapshot = shallowRef<NativeTerminalSettingsSnapshot | null>(null);
  const available = ref(false);
  const historyEpoch = ref(0);
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
  function historyChanged() { historyEpoch.value++; }
  async function refresh() {
    if (!isTauri()) return;
    if (inFlight) return inFlight;
    const generation = lifecycle;
    const request = (async () => {
      try {
        const settings = await getNativeTerminalSettings();
        if (generation !== lifecycle) return;
        applySettings(settings);
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
    clearTimeout(timer);
    timer = undefined;
    available.value = false;
    historyEpoch.value++;
  }
  return { settingsSnapshot, available, historyEpoch, historyChanged, applySettings, refresh, start, stop };
});
