import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { SftpFilePreviewRequest, SftpRemotePath, SftpRemoteObjectPrecondition } from "./core-api/generated/core-api";
export type ToolTarget =
  | { kind: "hostEditor"; hostId: string | null; title: string; initialSection?: "connection" | "classification" | "connectionRoute" | "algorithms" | "loginAutomation" | "connectionHealth" | "advanced" }
  | { kind: "desktopEditor"; profileId: string | null; title: string }
  | { kind: "sftpFile"; title: string; request: SftpFilePreviewRequest; path: SftpRemotePath; precondition: SftpRemoteObjectPrecondition; tail: boolean };
export function openToolWindow(target: ToolTarget): Promise<void> { return invoke("tool_window_open", { target }); }
export function onToolWindowChanged(callback: (kind: string) => void): Promise<() => void> {
  return listen<string>("tool-window-changed", (event) => callback(event.payload));
}
export function notifyToolWindowChanged(): Promise<void> { return invoke("tool_window_changed"); }
export function closeToolWindow(): Promise<void> { return invoke("tool_window_close"); }
