import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { installLinuxWindowShape } from "./linux-window-shape";
import { detectDesktopPlatform } from "./platform";
import { canvasBackgroundRgb } from "./workspace-tab-paint";

export type NativeBackgroundTarget = "window-and-webview" | "webview";

/**
 * Keeps the native background on the page canvas colour of the current theme, so an
 * area exposed before the page repaints (live resize, a first frame, a re-shown Tab
 * view) never flashes the platform default white. A top-level window sets both its
 * window and its WebView; a Tab child WebView sets only itself. The colour is always
 * opaque: WebView2 accepts only alpha 0 or 255. On macOS a WebView colour also stops
 * WKWebView drawing its own background; the page body still paints the canvas.
 */
export function startNativeBackgroundSync(target: NativeBackgroundTarget): () => void {
  if (!isTauri() || typeof MutationObserver !== "function") return () => undefined;
  // An opaque native background would fill the transparent window behind the rounded corners.
  if (detectDesktopPlatform() === "linux") return installLinuxWindowShape(target === "webview" ? "tab" : "window");
  let applied = "";
  let writes = Promise.resolve();
  const sync = () => {
    const rgb = canvasBackgroundRgb();
    const key = rgb?.join(",") ?? "";
    if (!rgb || key === applied) return;
    applied = key;
    const color: [number, number, number, number] = [rgb[0], rgb[1], rgb[2], 255];
    writes = writes.then(async () => {
      if (target === "window-and-webview") await getCurrentWindow().setBackgroundColor(color);
      await getCurrentWebview().setBackgroundColor(color);
    }).catch(() => {
      // A later theme change retries; the page itself still paints the right canvas.
      if (applied === key) applied = "";
    });
  };
  // Themes change `data-theme` and the inline custom properties on the root element.
  const observer = new MutationObserver(sync);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "style", "class"] });
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  media?.addEventListener?.("change", sync);
  sync();
  return () => {
    observer.disconnect();
    media?.removeEventListener?.("change", sync);
  };
}
