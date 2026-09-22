import { defineStore } from "pinia";
import { onScopeDispose, readonly, ref } from "vue";
import { normalizeHostMarker, type HostMarker } from "../host-markers";

export const HOST_MARKERS_STORAGE_KEY = "norishell.host-markers.v1";
type Preferences = { version: 1; enabled: boolean; markers: Record<string, HostMarker> };
export type HostMarkerSaveResult = "saved" | "invalid" | "storage-error";
function read(): Preferences {
  const fallback: Preferences = { version: 1, enabled: true, markers: {} };
  try {
    const value = JSON.parse(localStorage.getItem(HOST_MARKERS_STORAGE_KEY) ?? "null");
    if (value?.version !== 1 || typeof value.enabled !== "boolean" || !value.markers || typeof value.markers !== "object" || Array.isArray(value.markers)) return fallback;
    const markers = Object.fromEntries(Object.entries(value.markers).flatMap(([id, input]) => {
      const marker = normalizeHostMarker(input);
      return id && marker ? [[id, marker]] : [];
    }));
    return { version: 1, enabled: value.enabled, markers };
  } catch { return fallback; }
}
export const useHostMarkersStore = defineStore("hostMarkers", () => {
  const initial = read();
  const enabled = ref(initial.enabled);
  const markers = ref(initial.markers);
  function reload() { const current = read(); enabled.value = current.enabled; markers.value = current.markers; }
  const onStorage = (event: StorageEvent) => { if (event.key === HOST_MARKERS_STORAGE_KEY || event.key === null) reload(); };
  window.addEventListener("storage", onStorage);
  onScopeDispose(() => window.removeEventListener("storage", onStorage));
  function commit(next: Preferences): HostMarkerSaveResult {
    try { localStorage.setItem(HOST_MARKERS_STORAGE_KEY, JSON.stringify(next)); }
    catch { return "storage-error"; }
    enabled.value = next.enabled;
    markers.value = next.markers;
    return "saved";
  }
  function get(hostId: string | null | undefined): HostMarker | null {
    return hostId && Object.hasOwn(markers.value, hostId) ? markers.value[hostId] ?? null : null;
  }
  function visibleMarker(hostId: string | null | undefined): HostMarker | null {
    return enabled.value ? get(hostId) : null;
  }
  function set(hostId: string, input: HostMarker | null): HostMarkerSaveResult {
    if (!hostId.trim()) return "invalid";
    const marker = input === null ? null : normalizeHostMarker(input);
    if (input !== null && !marker) return "invalid";
    reload();
    const next = { ...markers.value };
    if (marker) Object.defineProperty(next, hostId, { value: marker, enumerable: true, configurable: true, writable: true });
    else delete next[hostId];
    if (JSON.stringify(next) === JSON.stringify(markers.value)) return "saved";
    return commit({ version: 1, enabled: enabled.value, markers: next });
  }
  function remove(hostId: string) { return set(hostId, null); }
  function setEnabled(value: boolean): HostMarkerSaveResult {
    reload();
    if (value === enabled.value) return "saved";
    return commit({ version: 1, enabled: value, markers: markers.value });
  }
  return { enabled: readonly(enabled), markers: readonly(markers), get, visibleMarker, set, remove, setEnabled };
});
