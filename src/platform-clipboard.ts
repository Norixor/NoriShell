import { isTauri } from "@tauri-apps/api/core";
import { readText as readNativeClipboardText } from "@tauri-apps/plugin-clipboard-manager";

/**
 * Reads plain text from the clipboard using the capability appropriate to the
 * current surface. Native WebViews use the Core-owned Tauri plugin because
 * browser Clipboard permissions do not reliably apply there.
 */
export function readText(): Promise<string> {
  return isTauri() ? readNativeClipboardText() : navigator.clipboard.readText();
}
