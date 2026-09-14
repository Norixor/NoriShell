import { invoke } from "@tauri-apps/api/core";
import type { NativeTrayAction, NativeTrayLocale } from "./generated/core-api";

export function takeNativeTrayAction(token: string): Promise<NativeTrayAction> {
  return invoke("tray_action_take", { request: { meta: { requestId: crypto.randomUUID() }, token } });
}
export function readyNativeTrayActions(locale: NativeTrayLocale): Promise<string[]> {
  return invoke("tray_actions_ready", { request: { meta: { requestId: crypto.randomUUID() }, locale } });
}
