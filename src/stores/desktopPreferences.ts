import { defineStore } from "pinia";
import { ref } from "vue";

import { getDesktopPreferences, replaceDesktopPreferences } from "../core-api/desktop-preferences";
import type { DesktopPreferences, DesktopPreferencesSnapshot } from "../core-api/generated/core-api";

export const useDesktopPreferencesStore = defineStore("desktopPreferences", () => {
  const snapshot = ref<DesktopPreferencesSnapshot | null>(null);
  const busy = ref(false);
  const error = ref<"loadFailed" | "saveFailed" | null>(null);

  async function refresh() {
    if (busy.value) return false;
    busy.value = true;
    error.value = null;
    try {
      snapshot.value = await getDesktopPreferences();
      return true;
    } catch {
      error.value = "loadFailed";
      return false;
    } finally { busy.value = false; }
  }

  async function replace(preferences: DesktopPreferences) {
    if (busy.value || !snapshot.value) return false;
    busy.value = true;
    error.value = null;
    const revision = snapshot.value.revision;
    // Submit one operation for the current snapshot only; conflicts cannot replay a merged result the user has not reviewed.
    try {
      snapshot.value = await replaceDesktopPreferences({ ...preferences }, revision);
      return true;
    } catch {
      error.value = "saveFailed";
      return false;
    } finally { busy.value = false; }
  }

  return { snapshot, busy, error, refresh, replace };
});
