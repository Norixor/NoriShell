import { createApp, watch } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
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
createApp(App).use(pinia).use(i18n).use(router).mount("#app");

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

void router.isReady().then(revealMainWindow, revealMainWindow);
