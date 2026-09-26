import { listen } from "@tauri-apps/api/event";

// Core emits this after a sync transaction changes saved connection metadata.
export function onSavedConnectionsChanged(callback: () => void): Promise<() => void> {
  return listen<null>("saved-connections-changed", callback);
}
