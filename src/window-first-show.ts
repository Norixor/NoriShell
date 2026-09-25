import { invoke } from "@tauri-apps/api/core";
import { nextTick } from "vue";

// A hidden WebView may throttle animation frames, so force style and layout
// after Vue mounts before asking Core to reveal the native window.
export async function revealWindowAfterMount(): Promise<void> {
  await nextTick();
  if (!document.body.isConnected) return;
  await document.fonts?.ready;
  await nextTick();
  window.getComputedStyle(document.body).getPropertyValue("background-color");
  document.body.getBoundingClientRect();
  try {
    await invoke("window_renderer_ready");
  } catch {
    // Core's bounded fallback reveals the window if the ready signal fails.
  }
}
