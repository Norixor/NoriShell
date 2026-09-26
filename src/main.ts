import { createApp, watch } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
import { initializeApplicationPreferences } from "./application-preferences-startup";
import { i18n } from "./locales";
import { router } from "./router";
import { useAppThemeStore } from "./stores/appTheme";
import { useUiStore } from "./stores/ui";
import { disableDefaultWebviewContextMenu } from "./webview-context-menu";
import { revealWindowAfterMount } from "./window-first-show";
import "./styles/tokens.css";
import "./styles/base.css";

const pinia = createPinia();
disableDefaultWebviewContextMenu();

async function revealMainWindow(): Promise<void> {
  const ui = useUiStore(pinia);
  const appTheme = useAppThemeStore(pinia);
  // Startup zoom and plugin theme resolution can change the initial layout.
  if (ui.uiZoomBusy || appTheme.loading) {
    await new Promise<void>((resolve) => {
      const stop = watch([() => ui.uiZoomBusy, () => appTheme.loading], ([zoomBusy, themeLoading]) => {
        if (!zoomBusy && !themeLoading) {
          stop();
          resolve();
        }
      }, { flush: "sync" });
    });
  }
  await revealWindowAfterMount();
}

async function start() {
  try {
    await initializeApplicationPreferences(pinia);
    createApp(App).use(pinia).use(i18n).use(router).mount("#app");
    await router.isReady().then(revealMainWindow, revealMainWindow);
  } catch {
    const root = document.querySelector("#app");
    if (root) {
      const message = document.createElement("p");
      message.textContent = navigator.language.startsWith("zh")
        ? "应用偏好读取失败。请重试；本机设置没有被覆盖。"
        : "Could not load application preferences. Retry; your local settings were not overwritten.";
      const retry = document.createElement("button");
      retry.textContent = navigator.language.startsWith("zh") ? "重试" : "Retry";
      retry.addEventListener("click", () => window.location.reload());
      root.replaceChildren(message, retry);
    }
    await revealWindowAfterMount();
  }
}

void start();
