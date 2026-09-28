import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * Opt-in startup timing for native Tab WebViews (see Core `tab_boot_trace`). A mark
 * carries only a stage name, the view role, a hash of the Tab id and a timestamp.
 */
let enabled: Promise<boolean> | null = null;

/** A short non-identifying correlation value; Tab ids may name plugins. */
export function tabBootTraceId(tabId: string): string {
  let hash = 0x811c9dc5;
  for (const character of tabId) {
    hash ^= character.codePointAt(0)!;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

export function markTabBoot(tabId: string, view: "shell" | "tab", stage: string): void {
  if (!isTauri() || !tabId) return;
  // Wall-clock time lets marks from separate WebViews line up in one log.
  const atMs = performance.timeOrigin + performance.now();
  enabled ??= invoke<boolean>("tab_boot_trace_enabled").catch(() => false);
  void enabled.then((on) => on
    ? invoke("tab_boot_trace", { mark: { trace: tabBootTraceId(tabId), view, stage, atMs } })
    : undefined).catch(() => undefined);
}
