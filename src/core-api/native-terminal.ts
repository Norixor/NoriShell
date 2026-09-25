import { invoke } from "@tauri-apps/api/core";
import {
  coreApiCommands,
  type NativeTerminalHistoryRecordRequest,
  type NativeTerminalHistoryEntry,
  type NativeTerminalHistoryScope,
  type NativeTerminalSettings,
  type NativeTerminalSettingsSnapshot,
} from "./generated/core-api";

const meta = () => ({ requestId: crypto.randomUUID() });

export function getNativeTerminalSettings(): Promise<NativeTerminalSettingsSnapshot> {
  return invoke(coreApiCommands.nativeTerminalSettingsGet, { request: { meta: meta() } });
}
export function replaceNativeTerminalSettings(settings: NativeTerminalSettings, expectedSettingsRevision: string): Promise<NativeTerminalSettingsSnapshot> {
  return invoke(coreApiCommands.nativeTerminalSettingsReplace, { request: { meta: meta(), settings, expectedSettingsRevision } });
}
export function recordNativeTerminalHistory(request: Omit<NativeTerminalHistoryRecordRequest, "meta">): Promise<boolean> {
  return invoke(coreApiCommands.nativeTerminalHistoryRecord, { request: { meta: meta(), ...request } });
}
export function listNativeTerminalHistory(input: { scope: NativeTerminalHistoryScope | null; query: string; limit?: number }): Promise<NativeTerminalHistoryEntry[]> {
  return invoke(coreApiCommands.nativeTerminalHistoryList, { request: { meta: meta(), limit: 50, ...input } });
}
export function deleteNativeTerminalHistory(entryId: string): Promise<void> {
  return invoke(coreApiCommands.nativeTerminalHistoryDelete, { request: { meta: meta(), entryId } });
}
export function clearNativeTerminalHistory(scope: NativeTerminalHistoryScope | null = null): Promise<void> {
  return invoke(coreApiCommands.nativeTerminalHistoryClear, { request: { meta: meta(), scope } });
}
export function pauseNativeTerminalHistory(paused: boolean): Promise<NativeTerminalSettingsSnapshot> {
  return invoke(coreApiCommands.nativeTerminalHistoryPause, { request: { meta: meta(), paused } });
}
