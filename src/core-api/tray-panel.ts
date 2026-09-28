import { invoke } from "@tauri-apps/api/core";
import type { NativeTrayPanelSnapshot } from "./generated/core-api";

/** Core-emitted, sequence-ordered panel visibility transitions (src-tauri tray_service/panel.rs). */
export const TRAY_PANEL_VISIBILITY_EVENT = "tray-panel-visibility";
export type TrayPanelVisibility = { sequence: number; visible: boolean };

const meta = () => ({ requestId: crypto.randomUUID() });

export function readTrayPanel(): Promise<NativeTrayPanelSnapshot> {
  return invoke("tray_panel_snapshot", { request: { meta: meta() } });
}

export function executeTrayPanel(token: string): Promise<void> {
  return invoke("tray_panel_execute", { request: { meta: meta(), token } });
}

export function hideTrayPanel(): Promise<void> {
  return invoke("tray_panel_hide", { request: { meta: meta() } });
}
