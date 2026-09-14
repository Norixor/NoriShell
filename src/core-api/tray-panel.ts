import { invoke } from "@tauri-apps/api/core";
import type { NativeTrayPanelSnapshot } from "./generated/core-api";

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
