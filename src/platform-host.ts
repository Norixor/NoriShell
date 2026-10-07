import { isTauri } from "@tauri-apps/api/core";

import { detectDesktopPlatform } from "./platform";

export type WorkspaceHostMode = "nativeTabs" | "singleDocument";

/** Whether Tauri IPC (Core commands and events) is available in this document. */
export function hasTauriIpc(): boolean {
  return isTauri();
}

/**
 * The phone layout is a platform fact, never a viewport measurement: a narrow
 * desktop window keeps the desktop frame, and a tablet-sized Android WebView keeps
 * the phone shell.
 */
export function isMobileLayout(): boolean {
  return detectDesktopPlatform() === "android";
}

/**
 * How Workspace Tabs are hosted. Desktop Tauri windows host every Tab in its own
 * native child WebView; Android (a single WebView, no child views) and the browser
 * preview keep Tabs and Panes inside this one document.
 */
export function workspaceHostMode(): WorkspaceHostMode {
  return hasTauriIpc() && !isMobileLayout() ? "nativeTabs" : "singleDocument";
}

/** Whether this document sits in a native desktop window with app-drawn or native chrome. */
export function hasNativeWindowChrome(): boolean {
  return hasTauriIpc() && !isMobileLayout();
}
