import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export const UI_ZOOM_LEVELS = [80, 90, 100, 110, 125, 150] as const;
export type UiZoom = typeof UI_ZOOM_LEVELS[number];

export function isUiZoom(value: unknown): value is UiZoom {
  return UI_ZOOM_LEVELS.some((level) => level === value);
}

export async function applyUiZoom(value: UiZoom): Promise<void> {
  const scale = value / 100;
  if (isTauri()) {
    await getCurrentWebview().setZoom(scale);
  } else {
    // Browser development previews use equivalent page scaling; native windows always call the WebView API.
    document.documentElement.style.zoom = String(scale);
  }
  document.documentElement.style.setProperty("--nvx-ui-zoom", String(scale));
  window.dispatchEvent(new Event("resize"));
}
