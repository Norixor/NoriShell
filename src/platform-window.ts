import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { requestWindowClose } from "./core-api/client";

export type WindowAction = "close" | "minimize" | "toggleMaximize" | "toggleFullscreen";

export interface WindowsCaptionHitRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export async function getNativeControlsInset(): Promise<number | null> {
  if (!isTauri()) return null;
  const inset = await invoke<number | null>("window_native_controls_inset");
  return typeof inset === "number" && Number.isFinite(inset) && inset > 0 ? inset : null;
}

export async function setNativeHeaderHeight(height: number): Promise<void> {
  if (!isTauri() || !Number.isFinite(height) || height <= 0) return;
  await invoke("window_set_native_header_height", { height });
}

export function physicalWindowsCaptionHitRegion(
  rect: Pick<DOMRect, "left" | "top" | "width" | "height">,
  scaleFactor: number,
): WindowsCaptionHitRegion | null {
  if (
    !Number.isFinite(scaleFactor) ||
    scaleFactor <= 0 ||
    !Number.isFinite(rect.left) ||
    !Number.isFinite(rect.top) ||
    !Number.isFinite(rect.width) ||
    !Number.isFinite(rect.height) ||
    rect.left < 0 ||
    rect.top < 0 ||
    rect.width <= 0 ||
    rect.height <= 0
  ) {
    return null;
  }

  return {
    x: Math.round(rect.left * scaleFactor),
    y: Math.round(rect.top * scaleFactor),
    width: Math.max(1, Math.round(rect.width * scaleFactor)),
    height: Math.max(1, Math.round(rect.height * scaleFactor)),
  };
}

export async function setWindowsMaximizeHitRegion(
  region: WindowsCaptionHitRegion | null,
): Promise<void> {
  if (!isTauri()) return;
  await invoke("window_set_windows_maximize_hit_region", { region });
}

/**
 * The single frontend bridge for platform window intentions.
 *
 * macOS native traffic lights remain owned by AppKit. This bridge is used by
 * the Windows caption component and by the shared draggable header gesture.
 */
export async function performWindowAction(action: WindowAction): Promise<void> {
  if (!isTauri()) return;
  if (action === "close") {
    await requestWindowClose();
    return;
  }

  const window = getCurrentWindow();
  if (action === "minimize") {
    await window.minimize();
    return;
  }
  if (action === "toggleFullscreen") {
    await window.setFullscreen(!await window.isFullscreen());
    return;
  }
  await window.toggleMaximize();
}
